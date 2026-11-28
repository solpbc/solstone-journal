// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Bounded transcript-only reads for one chronicle segment.

//! This module intentionally does not share the broad day-clustering surface:
//! it has no percept, browser, or talent selection flags.

mod long_message;
pub use long_message::{MessageBounds, TurnPart};

use std::collections::{BTreeMap, BTreeSet, hash_map::DefaultHasher};
use std::fs::{self, File};
use std::hash::{Hash, Hasher};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use serde_json::Value;
use solstone_core_format::content::conversation::{ConversationMedium, file_medium};
use solstone_core_format::content::{
    ContentResolution, Family, RawPerceptFamily, classify, produce_chunks,
    produce_raw_percept_chunks,
};
use solstone_core_journal_io::paths::{Segment, segment_origin};
use solstone_core_processing_record::is_transcribe_output;

use crate::possible_name_labels::label_line;
use crate::{
    PossibleNameLabels, VoiceNames, load_who_spoke, next_statement_ordinal, speaker_label_source,
};

/// Who an agent's transcript line names: only a speaker the journal's voice
/// gate names (the same gate an activity's Story reads), with the owner's own
/// recognized lines named by `owner`, so an agent can tell them apart.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AgentSpeakers {
    pub voices: VoiceNames,
    pub owner: String,
}

/// Read attribution from an imported conversation only while the retained
/// source is positively identified as speech. A neighboring audio file alone
/// does not make the writer of a message a speaker.
pub fn read_spoken_conversation<T>(
    path: &Path,
    read: impl FnOnce(&mut File) -> std::io::Result<T>,
) -> Result<Option<T>, SegmentTranscriptReadError> {
    let parent = path.parent().ok_or(SegmentTranscriptReadError::Io)?;
    let name = path
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(SegmentTranscriptReadError::Io)?;
    let relative = format!("{}/{name}", segment_origin(parent));
    long_message::read_if_medium(path, &relative, ConversationMedium::Spoken, |file, _| {
        file.rewind().map_err(|_| SegmentTranscriptReadError::Io)?;
        read(file).map_err(|_| SegmentTranscriptReadError::Io)
    })
}

/// Consume the retained conversation with its resolved medium. Local readers
/// can skip a written or unresolved body without opening the file again.
pub fn read_talent_conversation<T>(
    path: &Path,
    relative: &str,
    read: impl FnOnce(&mut File, ConversationMedium) -> std::io::Result<T>,
) -> Result<T, SegmentTranscriptReadError> {
    long_message::read_classified(path, relative, None, |file, _, medium| {
        file.rewind().map_err(|_| SegmentTranscriptReadError::Io)?;
        read(file, medium).map_err(|_| SegmentTranscriptReadError::Io)
    })?
    .ok_or(SegmentTranscriptReadError::Changed)
}

/// Ordinary source window. A historical oversized written turn may take one
/// initial record scan of up to 32 MiB, then 8 KiB per continuation.
pub const MAX_TRANSCRIPT_PAGE_BYTES: usize = 64 * 1024;
/// Largest number of transcript entries returned for one transcript page.
pub const MAX_TRANSCRIPT_PAGE_ITEMS: usize = 100;
pub const POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE: &str =
    "possible speaker change within this sentence";

/// Parsed wall-clock times and formatted display range for a chronicle segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentClock {
    pub label: String,
    pub start_seconds: u32,
    pub exclusive_end_seconds: u32,
}

impl SegmentClock {
    pub fn overlaps(&self, from: u32, to: u32) -> bool {
        if self.start_seconds == self.exclusive_end_seconds {
            return (from..to).contains(&self.start_seconds);
        }
        self.start_seconds.max(from) < self.exclusive_end_seconds.min(to)
    }
}

pub fn segment_clock(directory_name: &str) -> Option<SegmentClock> {
    let key = solstone_core_format::segment::segment_key(directory_name)?;
    let (time_part, length_part) = key.split_once('_')?;
    if time_part.len() != 6 || !time_part.bytes().all(|byte| byte.is_ascii_digit()) {
        return None;
    }
    let hour = time_part[0..2].parse::<u32>().ok()?;
    let minute = time_part[2..4].parse::<u32>().ok()?;
    let second = time_part[4..6].parse::<u32>().ok()?;
    if hour > 23 || minute > 59 || second > 59 {
        return None;
    }
    let duration = length_part.parse::<u32>().ok()?;
    let start_seconds = hour * 3600 + minute * 60 + second;
    let exclusive_end_seconds = start_seconds.saturating_add(duration).min(86400);
    let display_end = start_seconds.saturating_add(duration).min(86399);
    let end_h = display_end / 3600;
    let end_m = (display_end % 3600) / 60;
    let label = format!("{hour:02}:{minute:02}-{end_h:02}:{end_m:02}");
    Some(SegmentClock {
        label,
        start_seconds,
        exclusive_end_seconds,
    })
}

/// Opaque-to-callers source version for one segment's approved transcript files.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptVersion(u64);

impl SegmentTranscriptVersion {
    /// Stable source-version value for binding an opaque continuation.
    #[must_use]
    pub fn fingerprint(&self) -> u64 {
        self.0
    }

    /// Reconstruct a cursor version previously obtained from this API.
    #[must_use]
    pub fn from_fingerprint(value: u64) -> Self {
        Self(value)
    }
}

/// Private continuation point within the approved transcript file list.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptCursor {
    pub source_index: usize,
    pub byte_offset: u64,
    pub statement_ordinal: u64,
    pub message: Option<MessageBounds>,
    pub version: SegmentTranscriptVersion,
}

/// One projected transcript entry.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptEntry {
    pub turn_part: Option<TurnPart>,
    pub text: String,
    /// Channel heading understood by source units. Speech stays `Transcript`.
    /// Chat, markdown, and other imported text use a non-speech channel.
    pub channel: &'static str,
}

/// One bounded page from a single segment.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentTranscriptPage {
    pub entries: Vec<SegmentTranscriptEntry>,
    /// Registry-settled content kind of the file each entry in `entries` came from.
    entry_content_kinds: Vec<Option<&'static str>>,
    pub next: Option<SegmentTranscriptCursor>,
    pub version: SegmentTranscriptVersion,
    pub untimed: bool,
    /// Kinds of material served to this reader from this segment, in order of
    /// appearance and deduplicated. Never names a kind the connection was not
    /// permitted to read from the segment.
    pub content_kinds: Vec<&'static str>,
    /// An admitted file could contain either speech or messages.
    pub unresolved_conversation: bool,
}

impl SegmentTranscriptPage {
    /// Each projected entry paired with the registry-settled content kind of
    /// the file its text came from.
    pub fn entries_with_content_kind(
        &self,
    ) -> impl Iterator<Item = (&SegmentTranscriptEntry, Option<&'static str>)> {
        self.entries
            .iter()
            .zip(self.entry_content_kinds.iter().copied())
    }
}

/// Failure while reading a bounded transcript page.
#[derive(Debug)]
pub enum SegmentTranscriptReadError {
    NoPermittedFiles,
    Changed,
    InvalidCursor,
    Io,
    RecordTooLarge,
}

fn peek_untimed(path: &Path) -> bool {
    let Ok(file) = File::open(path) else {
        return false;
    };
    use std::io::BufRead;
    let mut reader = std::io::BufReader::new(file.take(MAX_TRANSCRIPT_PAGE_BYTES as u64));
    let mut line = String::new();
    if reader.read_line(&mut line).is_err() {
        return false;
    }
    if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(line.trim()) {
        map.get("untimed") == Some(&Value::Bool(true))
    } else {
        false
    }
}

/// Read one bounded page of approved transcript text from exactly one segment.
///
/// The caller owns authorization. This function only projects transcript input;
/// it never walks a day, reads unrelated percept/browser/talent content, or applies scope.
/// Segment-local speaker labels supply names only, and only as `speakers` allows;
/// the entity profile is never projected. Without `speakers`, lines keep the
/// stored diarization label.
///
/// `possible_names` are the labels this reader may show; the caller decides
/// which (`solstone-core-possible-names`). Each is inserted after the speech
/// it labels, and the set is part of the page version, so a continuation never
/// mixes pages from before and after it changed.
pub fn read_segment_transcript_page(
    segment: &Segment,
    cursor: Option<&SegmentTranscriptCursor>,
    speakers: Option<&AgentSpeakers>,
    possible_names: Option<&PossibleNameLabels>,
) -> Result<SegmentTranscriptPage, SegmentTranscriptReadError> {
    read_segment_transcript_page_leaving_out(
        segment,
        cursor,
        speakers,
        possible_names,
        &BTreeSet::new(),
    )
}

/// [`read_segment_transcript_page`], leaving out the body rows whose ids are
/// in `left_out`: the rows of a day of body data the owner hid, which the
/// caller works out. What is left out is part of the page version, so a
/// continuation never mixes pages from before and after it changed. With
/// nothing left out, the page and its version are the plain read's.
pub fn read_segment_transcript_page_leaving_out(
    segment: &Segment,
    cursor: Option<&SegmentTranscriptCursor>,
    speakers: Option<&AgentSpeakers>,
    possible_names: Option<&PossibleNameLabels>,
    left_out: &BTreeSet<String>,
) -> Result<SegmentTranscriptPage, SegmentTranscriptReadError> {
    read_segment_transcript_page_with_permissions(
        segment,
        cursor,
        speakers,
        possible_names,
        left_out,
        None,
    )
}

/// Connection-specific rendering policy for `body.jsonl` rows (for example,
/// gating workout GPS coordinates on the connection's `places` precision).
/// `policy_tag` is mixed into the segment page version so a precision change
/// invalidates stale continuation cursors.
pub struct BodyReadPolicy<'a> {
    pub policy_tag: u8,
    pub render_row: &'a dyn Fn(&solstone_core_format::content::body_records::BodyRow) -> String,
}

/// Project only files the connection may read, after segment-route selection.
/// Filter before numbering, versioning, paging or reading any file contents.
pub fn read_segment_transcript_page_with_permissions(
    segment: &Segment,
    cursor: Option<&SegmentTranscriptCursor>,
    speakers: Option<&AgentSpeakers>,
    possible_names: Option<&PossibleNameLabels>,
    left_out: &BTreeSet<String>,
    permissions: Option<&BTreeSet<solstone_core_format::content::AdmittedCategory>>,
) -> Result<SegmentTranscriptPage, SegmentTranscriptReadError> {
    read_segment_transcript_page_with_body_policy(
        segment,
        cursor,
        speakers,
        possible_names,
        left_out,
        permissions,
        None,
    )
}

/// [`read_segment_transcript_page_with_permissions`] with an optional body-row
/// rendering policy for agent egress controls.
pub fn read_segment_transcript_page_with_body_policy(
    segment: &Segment,
    cursor: Option<&SegmentTranscriptCursor>,
    speakers: Option<&AgentSpeakers>,
    possible_names: Option<&PossibleNameLabels>,
    left_out: &BTreeSet<String>,
    permissions: Option<&BTreeSet<solstone_core_format::content::AdmittedCategory>>,
    body_policy: Option<&BodyReadPolicy<'_>>,
) -> Result<SegmentTranscriptPage, SegmentTranscriptReadError> {
    let PermittedFiles {
        paths: files,
        media,
        content_kinds,
    } = permitted_transcript_files(segment, permissions)?;
    let withhold_womens_health = permissions.is_some_and(|allowed| {
        !allowed.contains(&solstone_core_format::content::AdmittedCategory::WomensHealth)
    });
    let unresolved_conversation = media.values().any(|m| *m == ConversationMedium::Unresolved);
    let segment_origin = segment_origin(segment.path());
    let labels = speakers.and_then(|speakers| {
        let labels = local_speaker_names(segment.path(), speakers)?;
        files
            .iter()
            .any(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(labels.0.as_str()))
            .then_some(labels)
    });
    // An agent read keeps one number on one voice through a continuous recording.
    let run_numbers = speakers
        .map(|_| crate::run_numbers_by_source(segment.path(), &files))
        .unwrap_or_default();
    let possible_names = possible_names.filter(|labels| {
        !labels.statements.is_empty()
            && files.iter().any(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(labels.file.as_str())
            })
    });
    let version = source_version(
        &files,
        &media,
        &labels,
        &run_numbers,
        possible_names,
        left_out,
        withhold_womens_health,
        body_policy.map_or(0, |policy| policy.policy_tag),
    )?;
    let untimed_files: BTreeSet<_> = files
        .iter()
        .filter(|path| {
            path.extension().and_then(|ext| ext.to_str()) == Some("jsonl") && peek_untimed(path)
        })
        .collect();
    let untimed = !untimed_files.is_empty();
    let (mut source_index, mut byte_offset, mut statement_ordinal) = match cursor {
        Some(cursor) if cursor.version != version => {
            return Err(SegmentTranscriptReadError::Changed);
        }
        Some(cursor) if cursor.source_index > files.len() => {
            return Err(SegmentTranscriptReadError::InvalidCursor);
        }
        Some(cursor) => (
            cursor.source_index,
            cursor.byte_offset,
            cursor.statement_ordinal,
        ),
        None => (0, 0, 0),
    };

    let mut message = cursor.and_then(|c| c.message);
    let mut entries = Vec::new();
    let mut entry_content_kinds = Vec::new();
    while source_index < files.len() && entries.len() < MAX_TRANSCRIPT_PAGE_ITEMS {
        let path = &files[source_index];
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let relative = format!("{segment_origin}/{name}");
        let medium = media.get(path).copied();
        let content_kind = match medium {
            Some(ConversationMedium::Unresolved) => None,
            Some(ConversationMedium::Written) => Some("messages"),
            Some(ConversationMedium::Spoken) => Some("transcripts"),
            None => Some(
                solstone_core_format::content::kinds::segment_file_permission(&relative).as_str(),
            ),
        };
        let (window, advanced, exhausted) = if message.is_some() {
            (Vec::new(), 0, false)
        } else if let Some(medium) = medium.filter(|_| name.ends_with(".jsonl")) {
            long_message::bound_read(path, &relative, medium, |file, _| {
                read_window_from(file, byte_offset)
            })?
        } else {
            read_window(path, byte_offset)?
        };
        if advanced == 0
            && !exhausted
            && matches!(
                media.get(path),
                Some(ConversationMedium::Written | ConversationMedium::Unresolved)
            )
            && matches!(
                classify(&format!("{segment_origin}/{name}")),
                ContentResolution::Indexed(Family::AiChat)
            )
        {
            let passage = long_message::bound_read(
                path,
                &relative,
                medium.ok_or(SegmentTranscriptReadError::Changed)?,
                |file, version| {
                    long_message::passage(
                        file,
                        version,
                        byte_offset,
                        message,
                        name,
                        &segment_origin,
                        untimed_files.contains(path),
                    )
                },
            )?;
            byte_offset = passage.next_offset;
            message = passage.bounds;
            if !passage.text.is_empty() {
                entry_content_kinds.push(content_kind);
                entries.push(SegmentTranscriptEntry {
                    channel: source_channel(&segment_origin, name, false, media.get(path).copied()),
                    text: passage.text,
                    turn_part: passage.part,
                });
            }
            if message.is_some() {
                break;
            }
            if byte_offset
                == fs::metadata(path)
                    .map_err(|_| SegmentTranscriptReadError::Io)?
                    .len()
            {
                source_index += 1;
                byte_offset = 0;
                statement_ordinal = 0;
            }
            break;
        }
        let speech = is_speech_transcript_name(name) || is_transcribe_output(path);
        let names = labels
            .as_ref()
            .filter(|(source, _)| {
                name.strip_suffix(".jsonl") == Some(source.as_str())
                    && !matches!(
                        classify(&format!("{segment_origin}/{name}")),
                        ContentResolution::Indexed(Family::AiChat)
                    )
            })
            .map(|(_, names)| names);
        let numbers = name
            .strip_suffix(".jsonl")
            .and_then(|stem| run_numbers.get(stem));
        let name_labels = possible_names.filter(|labels| {
            labels.file == name
                && speech
                && !matches!(
                    classify(&format!("{segment_origin}/{name}")),
                    ContentResolution::Indexed(Family::AiChat)
                )
        });
        let mut consumed = 0;
        let mut stopped_in_window = false;
        for (line, line_end) in complete_lines(&window)? {
            let sentence_id = next_statement_ordinal(line, &mut statement_ordinal);
            let named_line = names
                .and_then(|names| {
                    let speaker = &names.get(&sentence_id?)?.display;
                    let mut value: Value = serde_json::from_str(line).ok()?;
                    value
                        .as_object_mut()?
                        .insert("speaker".into(), Value::String(speaker.clone()));
                    Some(value.to_string())
                })
                .or_else(|| numbers.and_then(|numbers| crate::renumber_line(line, numbers)));
            let named = named_line.as_deref().unwrap_or(line);
            let labelled_line = name_labels
                .and_then(|labels| label_line(named, labels.statements.get(&sentence_id?)?));
            let text = if is_left_out(name, line, left_out, withhold_womens_health) {
                String::new()
            } else {
                project_line(
                    &segment_origin,
                    name,
                    speech,
                    untimed_files.contains(path),
                    labelled_line.as_deref().unwrap_or(named),
                    body_policy,
                )
            };
            if !text.trim().is_empty() {
                entry_content_kinds.push(content_kind);
                entries.push(SegmentTranscriptEntry {
                    turn_part: None,
                    channel: source_channel(
                        &segment_origin,
                        name,
                        speech,
                        media.get(path).copied(),
                    ),
                    text,
                });
            }
            consumed = line_end;
            if entries.len() == MAX_TRANSCRIPT_PAGE_ITEMS {
                stopped_in_window = consumed < advanced;
                break;
            }
        }
        byte_offset = byte_offset.saturating_add(
            (if stopped_in_window {
                consumed
            } else {
                advanced
            }) as u64,
        );
        if exhausted && !stopped_in_window {
            source_index += 1;
            byte_offset = 0;
            statement_ordinal = 0;
        }
        if advanced == 0 && !exhausted {
            return Err(SegmentTranscriptReadError::RecordTooLarge);
        }
    }

    let next = (source_index < files.len()).then(|| SegmentTranscriptCursor {
        source_index,
        byte_offset,
        statement_ordinal,
        version: version.clone(),
        message,
    });
    Ok(SegmentTranscriptPage {
        entries,
        entry_content_kinds,
        next,
        version,
        untimed,
        content_kinds,
        unresolved_conversation,
    })
}

/// Whether `line` of the file `name` is a body row the caller leaves out or
/// withholds (such as Women's Health rows when `womens_health` is not granted).
/// A line that doesn't read as a row is never one.
fn is_left_out(
    name: &str,
    line: &str,
    left_out: &BTreeSet<String>,
    withhold_womens_health: bool,
) -> bool {
    if name != solstone_core_format::content::body_records::BODY_RECORDS_FILE {
        return false;
    }
    if left_out.is_empty() && !withhold_womens_health {
        return false;
    }
    serde_json::from_str::<solstone_core_format::content::body_records::BodyRow>(line).is_ok_and(
        |row| {
            left_out.contains(&row.id)
                || (withhold_womens_health
                    && solstone_core_format::content::body_records::is_womens_health_row(&row))
        },
    )
}

/// One permitted file inside a segment, paired with its admitted content kind,
/// for lightweight cross-source summaries without paginating transcript rows.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PermittedSummaryFile {
    pub path: PathBuf,
    pub kind: solstone_core_format::content::AdmittedCategory,
}

/// List every transcript/import file in `segment` that `permissions` admits,
/// plus whether `segment` holds permitted screen-text material (either a stored
/// screen description when `screen_text` is granted, or screen media when
/// `screenshots` is also granted).
pub fn permitted_segment_files_for_summary(
    segment: &Segment,
    permissions: &BTreeSet<solstone_core_format::content::AdmittedCategory>,
) -> Result<(Vec<PermittedSummaryFile>, bool), SegmentTranscriptReadError> {
    use solstone_core_format::content::{AdmittedCategory, kinds::segment_file_permission};

    let files = transcript_files(segment.path())?;
    let origin = segment_origin(segment.path());
    let mut permitted = Vec::new();
    for path in files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let relative = format!("{origin}/{name}");
        let medium = file_medium(&path, &relative);
        let ordinary = segment_file_permission(&relative);
        let required: &[_] = match medium {
            Some(medium) => medium.required_permissions(),
            None => std::slice::from_ref(&ordinary),
        };
        if !required.iter().all(|kind| permissions.contains(kind)) {
            continue;
        }
        let kind = match required {
            [single] => *single,
            _ => ordinary,
        };
        permitted.push(PermittedSummaryFile { path, kind });
    }
    let already_screen = permitted
        .iter()
        .any(|file| file.kind == AdmittedCategory::ScreenText);
    let has_screen = !already_screen
        && permissions.contains(&AdmittedCategory::ScreenText)
        && (has_screen_description(segment.path())
            || (permissions.contains(&AdmittedCategory::Screenshots)
                && has_screen_media(segment.path(), &origin)?));
    Ok((permitted, has_screen))
}

/// Whether this segment can supply a transcript page under these permissions.
pub fn segment_has_permitted_files(
    segment: &Segment,
    permissions: &BTreeSet<solstone_core_format::content::AdmittedCategory>,
) -> bool {
    permitted_transcript_files(segment, Some(permissions))
        .is_ok_and(|files| !files.paths.is_empty() || !files.content_kinds.is_empty())
}

fn has_screen_media(segment: &Path, origin: &str) -> Result<bool, SegmentTranscriptReadError> {
    Ok(has_screen_description(segment)
        || fs::read_dir(segment)
            .map_err(|_| SegmentTranscriptReadError::Io)?
            .filter_map(Result::ok)
            .any(|entry| {
                entry.file_type().is_ok_and(|kind| kind.is_file())
                    && entry
                        .file_name()
                        .to_str()
                        .and_then(|name| {
                            solstone_core_format::screenshots::screen_media_stem(origin, name)
                        })
                        .is_some()
            }))
}

struct PermittedFiles {
    paths: Vec<PathBuf>,
    media: BTreeMap<PathBuf, ConversationMedium>,
    content_kinds: Vec<&'static str>,
}

fn permitted_transcript_files(
    segment: &Segment,
    permissions: Option<&BTreeSet<solstone_core_format::content::AdmittedCategory>>,
) -> Result<PermittedFiles, SegmentTranscriptReadError> {
    use solstone_core_format::content::{AdmittedCategory, kinds::segment_file_permission};
    let mut files = transcript_files(segment.path())?;
    let origin = segment_origin(segment.path());
    let mut screen_only = false;
    let was_empty = files.is_empty();
    let mut media = BTreeMap::new();
    let mut content_kinds = Vec::new();
    files.retain(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let relative = format!("{origin}/{name}");
        let medium = file_medium(path, &relative);
        let ordinary = segment_file_permission(&relative);
        let required: &[_] = match medium {
            Some(medium) => medium.required_permissions(),
            None => std::slice::from_ref(&ordinary),
        };
        if permissions.is_some_and(|allowed| !required.iter().all(|kind| allowed.contains(kind))) {
            return false;
        }
        if let Some(medium) = medium {
            media.insert(path.clone(), medium);
        }
        if let [kind] = required {
            let label = kind.as_str();
            if !content_kinds.contains(&label) {
                content_kinds.push(label);
            }
        }
        true
    });
    if let Some(permissions) = permissions {
        if files.is_empty() && (!was_empty || !permissions.contains(&AdmittedCategory::Transcripts))
        {
            // A permitted screen can have only media and its description, or
            // share a segment with speech this connection cannot read.
            let screen = permissions.contains(&AdmittedCategory::Screenshots)
                && has_screen_media(segment.path(), &origin)?;
            if !screen {
                return Err(SegmentTranscriptReadError::NoPermittedFiles);
            }
            screen_only = true;
        } else if files.is_empty()
            && permissions.contains(&AdmittedCategory::Screenshots)
            && has_screen_media(segment.path(), &origin)?
        {
            screen_only = true;
        }
    }
    if screen_only && !content_kinds.contains(&AdmittedCategory::ScreenText.as_str()) {
        content_kinds.push(AdmittedCategory::ScreenText.as_str());
    }
    Ok(PermittedFiles {
        paths: files,
        media,
        content_kinds,
    })
}

fn has_screen_description(segment: &Path) -> bool {
    let talents = segment.join("talents");
    fs::symlink_metadata(&talents).is_ok_and(|meta| meta.is_dir())
        && ["screen.md", "screen_text.md"]
            .iter()
            .any(|name| fs::symlink_metadata(talents.join(name)).is_ok_and(|meta| meta.is_file()))
}

fn transcript_files(segment: &Path) -> Result<Vec<PathBuf>, SegmentTranscriptReadError> {
    let mut files = Vec::new();
    for entry in fs::read_dir(segment).map_err(|_| SegmentTranscriptReadError::Io)? {
        let entry = entry.map_err(|_| SegmentTranscriptReadError::Io)?;
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|name| name.to_str()) else {
            continue;
        };
        if is_transcript_name(name) || is_transcribe_output(&path) {
            let meta = fs::symlink_metadata(&path).map_err(|_| SegmentTranscriptReadError::Io)?;
            if !meta.file_type().is_file() {
                return Err(SegmentTranscriptReadError::Io);
            }
            files.push(path);
        }
    }
    files.sort();
    Ok(files)
}

fn is_transcript_name(name: &str) -> bool {
    !solstone_core_format::content::segment_file_offers(
        name,
        solstone_core_format::content::SegmentReader::Agent,
        solstone_core_format::content::FileFacts::default(),
    )
    .is_empty()
}

#[derive(Clone, Debug, Eq, PartialEq, Hash)]
struct ResolvedSpeakerLabel {
    display: String,
    is_owner: bool,
    entity_id: Option<String>,
}

#[allow(clippy::too_many_arguments)]
fn source_version(
    files: &[PathBuf],
    media: &BTreeMap<PathBuf, ConversationMedium>,
    labels: &Option<(String, BTreeMap<i64, ResolvedSpeakerLabel>)>,
    run_numbers: &BTreeMap<String, BTreeMap<i64, i64>>,
    possible_names: Option<&PossibleNameLabels>,
    left_out: &BTreeSet<String>,
    withhold_womens_health: bool,
    body_policy_tag: u8,
) -> Result<SegmentTranscriptVersion, SegmentTranscriptReadError> {
    let mut hasher = DefaultHasher::new();
    labels.hash(&mut hasher);
    run_numbers.hash(&mut hasher);
    if let Some(possible_names) = possible_names {
        possible_names.hash(&mut hasher);
    }
    if !left_out.is_empty() {
        left_out.hash(&mut hasher);
    }
    let has_body_records = files.iter().any(|path| {
        path.file_name().and_then(|name| name.to_str())
            == Some(solstone_core_format::content::body_records::BODY_RECORDS_FILE)
    });
    if has_body_records && (withhold_womens_health || body_policy_tag != 0) {
        withhold_womens_health.hash(&mut hasher);
        body_policy_tag.hash(&mut hasher);
    }
    for path in files {
        let metadata = fs::metadata(path).map_err(|_| SegmentTranscriptReadError::Io)?;
        path.file_name().hash(&mut hasher);
        if let Some(medium) = media.get(path) {
            medium.hash(&mut hasher);
        }
        metadata.len().hash(&mut hasher);
        let modified = metadata
            .modified()
            .map_err(|_| SegmentTranscriptReadError::Io)?
            .duration_since(std::time::UNIX_EPOCH)
            .map_err(|_| SegmentTranscriptReadError::Io)?;
        modified.as_secs().hash(&mut hasher);
        modified.subsec_nanos().hash(&mut hasher);
    }
    Ok(SegmentTranscriptVersion(hasher.finish()))
}

// Resolve only names used by this segment, through the voice gate: a name the
// journal inferred from what was said, or a declined owner match, names no one.
// The owner's own answers apply; both files are read within the page bound.
fn local_speaker_names(
    segment: &Path,
    speakers: &AgentSpeakers,
) -> Option<(String, BTreeMap<i64, ResolvedSpeakerLabel>)> {
    let (source, _) = speaker_label_source(segment);
    let source = source?;
    let labels = load_who_spoke(segment, Some(&source), Some(MAX_TRANSCRIPT_PAGE_BYTES))
        .rows
        .iter()
        .filter_map(|row| {
            let id = row.get("sentence_id")?.as_i64()?;
            let name = speakers.voices.name_for(row)?;
            let entity_id = row
                .get("speaker")
                .and_then(Value::as_str)
                .map(str::to_owned);
            let is_owner = entity_id.as_deref() == Some(speakers.voices.principal.as_str());
            let display = if is_owner {
                speakers.owner.clone()
            } else {
                name
            };
            Some((
                id,
                ResolvedSpeakerLabel {
                    display,
                    is_owner,
                    entity_id,
                },
            ))
        })
        .collect::<BTreeMap<_, _>>();
    (!labels.is_empty()).then_some((source, labels))
}

fn read_window(
    path: &Path,
    offset: u64,
) -> Result<(Vec<u8>, usize, bool), SegmentTranscriptReadError> {
    let meta = fs::symlink_metadata(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    if !meta.file_type().is_file() {
        return Err(SegmentTranscriptReadError::Io);
    }
    let mut file = File::open(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    read_window_from(&mut file, offset)
}

fn read_window_from(
    file: &mut File,
    offset: u64,
) -> Result<(Vec<u8>, usize, bool), SegmentTranscriptReadError> {
    let length = file
        .metadata()
        .map_err(|_| SegmentTranscriptReadError::Io)?
        .len();
    if offset > length {
        return Err(SegmentTranscriptReadError::Changed);
    }
    file.seek(SeekFrom::Start(offset))
        .map_err(|_| SegmentTranscriptReadError::Io)?;
    let mut bytes = Vec::with_capacity(MAX_TRANSCRIPT_PAGE_BYTES);
    file.take(MAX_TRANSCRIPT_PAGE_BYTES as u64)
        .read_to_end(&mut bytes)
        .map_err(|_| SegmentTranscriptReadError::Io)?;
    let exhausted = offset + bytes.len() as u64 == length;
    let advanced = if exhausted {
        bytes.len()
    } else {
        bytes
            .iter()
            .rposition(|byte| *byte == b'\n')
            .map(|index| index + 1)
            .unwrap_or(0)
    };
    bytes.truncate(advanced);
    Ok((bytes, advanced, exhausted))
}

fn complete_lines(bytes: &[u8]) -> Result<Vec<(&str, usize)>, SegmentTranscriptReadError> {
    let text = std::str::from_utf8(bytes).map_err(|_| SegmentTranscriptReadError::Io)?;
    let mut end = 0;
    let lines = text
        .split_inclusive('\n')
        .map(|raw| {
            end += raw.len();
            let line = raw.strip_suffix('\n').unwrap_or(raw);
            let line = line.strip_suffix('\r').unwrap_or(line);
            (line, end)
        })
        .collect::<Vec<_>>();
    Ok(lines)
}

fn append_speaker_change_warning(text: &str, line: &str) -> String {
    let suspect = serde_json::from_str::<Value>(line)
        .ok()
        .and_then(|v| v.get("turn_boundary_suspect").and_then(Value::as_bool))
        == Some(true);
    if suspect && !text.is_empty() {
        format!("{text} [{POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE}]")
    } else {
        text.to_owned()
    }
}

fn stream_name(segment_origin: &str) -> Option<&str> {
    let mut parts = segment_origin.split('/');
    let _day = parts.next()?;
    let stream = parts.next()?;
    let _segment = parts.next()?;
    parts.next().is_none().then_some(stream)
}

fn is_speech_transcript_name(name: &str) -> bool {
    solstone_core_format::content::speech_transcript_name(name)
}

/// The source-unit channel for one approved transcript file. Classification is
/// the same path `project_line` already resolves; this does not open another file.
fn source_channel(
    segment_origin: &str,
    name: &str,
    speech: bool,
    medium: Option<ConversationMedium>,
) -> &'static str {
    use solstone_core_format::content::{AdmittedCategory, kinds::segment_file_permission};
    let rel = format!("{segment_origin}/{name}");
    let stream = stream_name(segment_origin);
    match medium {
        Some(ConversationMedium::Unresolved) => return "Imported Conversation",
        Some(ConversationMedium::Spoken)
            if matches!(classify(&rel), ContentResolution::Indexed(Family::AiChat)) =>
        {
            return "Imported Transcript";
        }
        Some(ConversationMedium::Written) => {
            return match stream {
                Some("import.chatgpt") => "ChatGPT Conversation",
                Some("import.claude") => "Claude Conversation",
                Some("import.gemini") => "Gemini Conversation",
                Some("import.email") => "Email",
                _ => "Imported Messages",
            };
        }
        _ => {}
    }
    match classify(&rel) {
        ContentResolution::Indexed(Family::Audio)
        | ContentResolution::Unindexed(RawPerceptFamily::Audio) => "Transcript",
        ContentResolution::Indexed(Family::BodyRecords) => "Body Records",
        _ if speech => "Transcript",
        _ => match stream {
            Some("import.obsidian") | Some("import.evernote") => "Note",
            Some("import.kindle") => "Highlights",
            Some("import.ics") => "Calendar Event",
            Some(stream) if solstone_core_format::obsidian::is_obsidian_stream_name(stream) => {
                "Note"
            }
            _ => match segment_file_permission(&rel) {
                AdmittedCategory::Notes => "Note",
                AdmittedCategory::Documents => "Document",
                AdmittedCategory::Transactions => "Transaction",
                AdmittedCategory::ScreenText => "Screenshot",
                AdmittedCategory::Photos => "Photo",
                AdmittedCategory::Calendar => "Calendar Event",
                AdmittedCategory::Transcripts
                    if stream.is_some_and(|stream| stream.starts_with("import.")) =>
                {
                    "Imported Transcript"
                }
                _ => "Imported Text",
            },
        },
    }
}

fn project_line(
    segment_origin: &str,
    name: &str,
    speech: bool,
    untimed: bool,
    line: &str,
    body_policy: Option<&BodyReadPolicy<'_>>,
) -> String {
    let rel = format!("{segment_origin}/{name}");
    match classify(&rel) {
        ContentResolution::Indexed(Family::Audio)
        | ContentResolution::Unindexed(RawPerceptFamily::Audio) => {
            let text = produce_raw_percept_chunks(RawPerceptFamily::Audio, &rel, line)
                .chunks
                .into_iter()
                .map(|chunk| chunk.content)
                .collect::<Vec<_>>()
                .join("\n");
            append_speaker_change_warning(&text, line)
        }
        ContentResolution::Indexed(Family::BodyRecords) => {
            if let Some(policy) = body_policy {
                return serde_json::from_str::<solstone_core_format::content::body_records::BodyRow>(
                    line,
                )
                .map_or_else(|_| line.to_owned(), |row| (policy.render_row)(&row));
            }
            solstone_core_format::content::format_file(
                solstone_core_format::content::FileFormat::Content(Family::BodyRecords),
                &rel,
                line,
                solstone_core_format::content::FormatterView::BodyLine,
            )
            .chunks
            .into_iter()
            .map(|chunk| chunk.content)
            .collect::<Vec<_>>()
            .join("\n")
        }
        ContentResolution::Indexed(Family::AiChat) => {
            let produced = produce_chunks(Family::AiChat, &rel, line);
            let start = serde_json::from_str::<Value>(line).ok().and_then(|row| {
                use solstone_core_format::content::conversation::{TurnTime, turn_time};
                if turn_time(stream_name(segment_origin), row.as_object()?) == TurnTime::Unknown {
                    return None;
                }
                row.get("start")?.as_str().map(str::to_owned)
            });
            produced
                .chunks
                .into_iter()
                .map(|chunk| {
                    if !untimed && let Some(ref start) = start {
                        format!("[{start}] {}", chunk.content)
                    } else {
                        chunk.content
                    }
                })
                .collect::<Vec<_>>()
                .join("\n")
        }
        _ => {
            if name == "workout.json" {
                produce_chunks(Family::Workout, name, line)
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<Vec<_>>()
                    .join("\n")
            } else if speech {
                let text = produce_raw_percept_chunks(RawPerceptFamily::Audio, &rel, line)
                    .chunks
                    .into_iter()
                    .map(|chunk| chunk.content)
                    .collect::<Vec<_>>()
                    .join("\n");
                append_speaker_change_warning(&text, line)
            } else {
                solstone_core_format::content::format_file(
                    solstone_core_format::content::FileFormat::Content(Family::Markdown),
                    &rel,
                    line,
                    solstone_core_format::content::FormatterView::LiteralMarkdown,
                )
                .chunks
                .into_iter()
                .map(|chunk| chunk.content)
                .collect::<Vec<_>>()
                .join("\n")
            }
        }
    }
}

/// Largest speech transcript file opened to verify and format an indexed search hit.
const MAX_INDEXED_SPEECH_FILE_BYTES: usize = 32 * 1024 * 1024;

/// One speech transcript line resolved live from disk with the same
/// speaker naming, run numbering, and possible-name labelling as `get_transcript`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSpeechLine {
    /// Full rendered line as `get_transcript` renders it (e.g. `[16:37:14] Nick Sorrell: ...`).
    pub text: String,
    /// Exact line wall clock from the JSONL record (e.g. `16:37:14`), omitted when untimed or absent.
    pub time: Option<String>,
    /// Display speaker resolved identically to `get_transcript`, omitted when unlabelled.
    pub speaker: Option<String>,
    /// True only when the journal's voice gate or owner assignment attributes this line to the owner.
    pub is_owner: bool,
    /// Voice-attributed or owner-assigned entity id for this speaker, when resolved by the voice gate.
    pub speaker_entity_id: Option<String>,
    /// True when the source file carries `"untimed": true` in its header.
    pub untimed: bool,
}

/// All permitted spoken transcript lines in one chronicle segment, in chronological file order.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SpokenSegmentLines {
    pub lines: Vec<ResolvedSpeechLine>,
    /// Distinct resolved display speaker labels present across this segment's spoken lines,
    /// in order of first appearance.
    pub speakers: Vec<String>,
    pub untimed: bool,
    pub version: SegmentTranscriptVersion,
}

fn is_spoken_transcript_file(
    path: &Path,
    relative: &str,
    medium: Option<ConversationMedium>,
) -> bool {
    match medium {
        Some(ConversationMedium::Spoken) => true,
        Some(ConversationMedium::Written | ConversationMedium::Unresolved) => false,
        None => {
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or_default();
            matches!(
                classify(relative),
                ContentResolution::Indexed(Family::Audio)
                    | ContentResolution::Unindexed(RawPerceptFamily::Audio)
            ) || is_speech_transcript_name(name)
                || is_transcribe_output(path)
        }
    }
}

/// Whether this segment contains at least one permitted spoken transcript file.
pub fn segment_has_spoken_transcript_files(
    segment: &Segment,
    permissions: &BTreeSet<solstone_core_format::content::AdmittedCategory>,
) -> bool {
    if !permissions.contains(&solstone_core_format::content::AdmittedCategory::Transcripts) {
        return false;
    }
    let Ok(permitted) = permitted_transcript_files(segment, Some(permissions)) else {
        return false;
    };
    let origin = segment_origin(segment.path());
    permitted.paths.iter().any(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let relative = format!("{origin}/{name}");
        is_spoken_transcript_file(path, &relative, permitted.media.get(path).copied())
    })
}

/// Read every permitted spoken transcript line from one segment, applying the
/// same voice gate, owner line labels, run numbering, and possible-name labels
/// as [`read_segment_transcript_page_with_permissions`].
///
/// Non-spoken files (notes, screen text, written chats, body records, images)
/// are ignored; returns [`SegmentTranscriptReadError::NoPermittedFiles`] when
/// the segment has no permitted spoken transcript files.
pub fn read_spoken_segment_lines(
    segment: &Segment,
    speakers: Option<&AgentSpeakers>,
    possible_names: Option<&PossibleNameLabels>,
    permissions: Option<&BTreeSet<solstone_core_format::content::AdmittedCategory>>,
) -> Result<SpokenSegmentLines, SegmentTranscriptReadError> {
    if permissions.is_some_and(|allowed| {
        !allowed.contains(&solstone_core_format::content::AdmittedCategory::Transcripts)
    }) {
        return Err(SegmentTranscriptReadError::NoPermittedFiles);
    }
    let PermittedFiles {
        paths: mut files,
        media,
        ..
    } = permitted_transcript_files(segment, permissions)?;
    let origin = segment_origin(segment.path());
    files.retain(|path| {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let relative = format!("{origin}/{name}");
        is_spoken_transcript_file(path, &relative, media.get(path).copied())
    });
    if files.is_empty() {
        return Err(SegmentTranscriptReadError::NoPermittedFiles);
    }
    let labels = speakers.and_then(|speakers| {
        let labels = local_speaker_names(segment.path(), speakers)?;
        files
            .iter()
            .any(|path| path.file_stem().and_then(|stem| stem.to_str()) == Some(labels.0.as_str()))
            .then_some(labels)
    });
    let run_numbers = speakers
        .map(|_| crate::run_numbers_by_source(segment.path(), &files))
        .unwrap_or_default();
    let possible_names = possible_names.filter(|labels| {
        !labels.statements.is_empty()
            && files.iter().any(|path| {
                path.file_name().and_then(|name| name.to_str()) == Some(labels.file.as_str())
            })
    });
    let empty_left_out = BTreeSet::new();
    let version = source_version(
        &files,
        &media,
        &labels,
        &run_numbers,
        possible_names,
        &empty_left_out,
        false,
        0,
    )?;

    let mut lines = Vec::new();
    let mut distinct_speakers = Vec::new();
    let mut seen_speakers = BTreeSet::new();
    let mut any_untimed = false;

    for path in &files {
        let name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or_default();
        let relative = format!("{origin}/{name}");
        let medium = media.get(path).copied();

        if medium == Some(ConversationMedium::Spoken) {
            let text = long_message::bound_read(
                path,
                &relative,
                ConversationMedium::Spoken,
                |file, _| read_bounded_utf8_from(file),
            )?;
            let untimed = text
                .lines()
                .next()
                .and_then(|first| serde_json::from_str::<Value>(first.trim()).ok())
                .and_then(|v| v.as_object().cloned())
                .is_some_and(|meta| meta.get("untimed") == Some(&Value::Bool(true)));
            any_untimed |= untimed;
            for raw in text.lines() {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    continue;
                }
                let projected = project_line(&origin, name, true, untimed, trimmed, None);
                if projected.trim().is_empty() {
                    continue;
                }
                let row = serde_json::from_str::<Value>(trimmed)
                    .ok()
                    .and_then(|v| v.as_object().cloned());
                let time = (!untimed)
                    .then(|| {
                        row.as_ref().and_then(|row| {
                            use solstone_core_format::content::conversation::{
                                TurnTime, turn_time,
                            };
                            if turn_time(stream_name(&origin), row) == TurnTime::Unknown {
                                return None;
                            }
                            row.get("start")
                                .and_then(Value::as_str)
                                .filter(|s| !s.is_empty())
                                .map(str::to_owned)
                        })
                    })
                    .flatten();
                let speaker = row.as_ref().and_then(|row| {
                    row.get("speaker")
                        .and_then(Value::as_str)
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                });
                if let Some(ref sp) = speaker
                    && seen_speakers.insert(sp.clone())
                {
                    distinct_speakers.push(sp.clone());
                }
                lines.push(ResolvedSpeechLine {
                    text: projected,
                    time,
                    speaker,
                    is_owner: false,
                    speaker_entity_id: None,
                    untimed,
                });
            }
            continue;
        }

        let text = read_bounded_utf8_path(path)?;
        let parsed_lines = parse_jsonl_objects_with_raw(&text);
        let metadata = parsed_lines
            .first()
            .filter(|(_, record)| !record.contains_key("start"))
            .map(|(_, record)| record.clone());
        let untimed = metadata
            .as_ref()
            .is_some_and(|meta| meta.get("untimed") == Some(&Value::Bool(true)));
        any_untimed |= untimed;

        if metadata.as_ref().is_some_and(|meta| {
            meta.get(solstone_core_format::content::granola::HEADER_KEY)
                .is_some_and(Value::is_object)
        }) {
            let produced = produce_chunks(Family::Audio, &relative, &text);
            for chunk in produced.chunks {
                if let Some(resolved) = resolve_granola_spoken_chunk(&chunk, untimed) {
                    if let Some(ref sp) = resolved.speaker
                        && seen_speakers.insert(sp.clone())
                    {
                        distinct_speakers.push(sp.clone());
                    }
                    lines.push(resolved);
                }
            }
            continue;
        }

        let file_labels = labels
            .as_ref()
            .filter(|(source, _)| name.strip_suffix(".jsonl") == Some(source.as_str()))
            .map(|(_, names)| names);
        let file_numbers = name
            .strip_suffix(".jsonl")
            .and_then(|stem| run_numbers.get(stem));
        let file_possible_names = possible_names.filter(|labels| labels.file == name);
        let mut statement_ordinal = 0_u64;
        for (index, (raw, _record)) in parsed_lines.into_iter().enumerate() {
            if index == 0 && metadata.is_some() {
                continue;
            }
            let Some(ordinal) = next_statement_ordinal(&raw, &mut statement_ordinal) else {
                continue;
            };
            if let Some(resolved) = render_speech_record(
                &relative,
                untimed,
                metadata.as_ref(),
                file_labels,
                file_numbers,
                file_possible_names,
                &raw,
                ordinal,
            ) {
                if let Some(ref sp) = resolved.speaker
                    && seen_speakers.insert(sp.clone())
                {
                    distinct_speakers.push(sp.clone());
                }
                lines.push(resolved);
            }
        }
    }

    Ok(SpokenSegmentLines {
        lines,
        speakers: distinct_speakers,
        untimed: any_untimed,
        version,
    })
}

fn read_bounded_utf8_path(path: &Path) -> Result<String, SegmentTranscriptReadError> {
    let meta = fs::symlink_metadata(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    if !meta.file_type().is_file() {
        return Err(SegmentTranscriptReadError::Io);
    }
    if meta.len() > MAX_INDEXED_SPEECH_FILE_BYTES as u64 {
        return Err(SegmentTranscriptReadError::RecordTooLarge);
    }
    let mut file = File::open(path).map_err(|_| SegmentTranscriptReadError::Io)?;
    read_bounded_utf8_from(&mut file)
}

fn read_bounded_utf8_from(file: &mut File) -> Result<String, SegmentTranscriptReadError> {
    let mut bytes = Vec::new();
    file.take(MAX_INDEXED_SPEECH_FILE_BYTES as u64 + 1)
        .read_to_end(&mut bytes)
        .map_err(|_| SegmentTranscriptReadError::Io)?;
    if bytes.len() > MAX_INDEXED_SPEECH_FILE_BYTES {
        return Err(SegmentTranscriptReadError::RecordTooLarge);
    }
    String::from_utf8(bytes).map_err(|_| SegmentTranscriptReadError::Io)
}

fn parse_jsonl_objects_with_raw(text: &str) -> Vec<(String, serde_json::Map<String, Value>)> {
    let mut parsed_lines = Vec::new();
    for raw in text.lines() {
        let trimmed = raw.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(Value::Object(map)) = serde_json::from_str::<Value>(trimmed) {
            parsed_lines.push((raw.to_owned(), map));
        }
    }
    parsed_lines
}

fn resolve_granola_spoken_chunk(
    chunk: &solstone_core_format::content::IndexChunk,
    untimed: bool,
) -> Option<ResolvedSpeechLine> {
    let source = chunk.source.as_ref()?;
    if !source.contains_key("source_ms") {
        return None;
    }
    let time = (!untimed)
        .then(|| {
            source
                .get("start")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .flatten();
    let speaker = source
        .get("speaker")
        .and_then(Value::as_str)
        .filter(|s| !s.is_empty())
        .map(str::to_owned);
    if chunk.content.trim().is_empty() {
        return None;
    }
    Some(ResolvedSpeechLine {
        text: chunk.content.clone(),
        time,
        speaker,
        is_owner: false,
        speaker_entity_id: None,
        untimed,
    })
}

#[allow(clippy::too_many_arguments)]
fn render_speech_record(
    normalized: &str,
    untimed: bool,
    metadata: Option<&serde_json::Map<String, Value>>,
    labels: Option<&BTreeMap<i64, ResolvedSpeakerLabel>>,
    run_numbers: Option<&BTreeMap<i64, i64>>,
    possible_names: Option<&PossibleNameLabels>,
    raw_line: &str,
    statement_ordinal: i64,
) -> Option<ResolvedSpeechLine> {
    let voice_label = labels.and_then(|names| names.get(&statement_ordinal));
    let named_line = voice_label
        .and_then(|label| {
            let mut value: Value = serde_json::from_str(raw_line).ok()?;
            value
                .as_object_mut()?
                .insert("speaker".into(), Value::String(label.display.clone()));
            Some(value.to_string())
        })
        .or_else(|| run_numbers.and_then(|numbers| crate::renumber_line(raw_line, numbers)));
    let named = named_line.as_deref().unwrap_or(raw_line);
    let labelled_line = possible_names
        .and_then(|labels| label_line(named, labels.statements.get(&statement_ordinal)?));
    let final_line = labelled_line.as_deref().unwrap_or(named);
    let final_record =
        serde_json::from_str::<Value>(final_line)
            .ok()
            .and_then(|value| match value {
                Value::Object(map) => Some(map),
                _ => None,
            })?;
    let speaker =
        solstone_core_format::content::audio_speaker_label(normalized, metadata, &final_record)
            .filter(|s| !s.is_empty());
    let time = (!untimed)
        .then(|| {
            final_record
                .get("start")
                .and_then(Value::as_str)
                .filter(|s| !s.is_empty())
                .map(str::to_owned)
        })
        .flatten();
    let mut pair = Vec::with_capacity(2);
    if let Some(meta) = metadata {
        pair.push(meta.clone());
    }
    pair.push(final_record);
    let rendered = solstone_core_format::content::produce_chunks_by_shape(
        Family::Audio,
        Some(normalized),
        &pair,
    )
    .chunks
    .into_iter()
    .map(|chunk| chunk.content)
    .collect::<Vec<_>>()
    .join("\n");
    let text = append_speaker_change_warning(&rendered, raw_line);
    if text.trim().is_empty() {
        return None;
    }
    Some(ResolvedSpeechLine {
        text,
        time,
        speaker,
        is_owner: voice_label.is_some_and(|label| label.is_owner),
        speaker_entity_id: voice_label.and_then(|label| label.entity_id.clone()),
        untimed,
    })
}

fn strip_leading_bracket_clock(text: &str) -> &str {
    let trimmed = text.trim();
    if let Some(after_open) = trimmed.strip_prefix('[')
        && let Some(close) = after_open.find(']')
        && close > 0
        && !after_open[..close].chars().any(char::is_whitespace)
    {
        after_open[close + 1..].trim_start()
    } else {
        trimmed
    }
}

/// Whether an indexed audio chunk string matches a live spoken body from disk,
/// allowing clock, `(source)` tag, or speaker-label prefix drift while refusing
/// any edited, truncated, or missing spoken text.
fn speech_chunk_matches_body(expected_indexed_content: &str, live_body: &str) -> bool {
    let live_body = live_body.trim();
    if live_body.is_empty() {
        return false;
    }
    let rest = strip_leading_bracket_clock(expected_indexed_content);
    if rest == live_body {
        return true;
    }
    let Some(before) = rest.strip_suffix(live_body) else {
        return false;
    };
    if let Some(speaker_prefix) = before.strip_suffix(": ") {
        return !speaker_prefix.is_empty()
            && !speaker_prefix.contains('\n')
            && !speaker_prefix.contains(": ");
    }
    if let Some(source_prefix) = before.strip_suffix(' ') {
        return source_prefix.starts_with('(')
            && source_prefix.ends_with(')')
            && !source_prefix.chars().any(char::is_whitespace);
    }
    false
}

fn granola_chunk_matches_expected(
    chunk: &solstone_core_format::content::IndexChunk,
    expected_indexed_content: &str,
) -> bool {
    if chunk.content.trim() == expected_indexed_content.trim() {
        return true;
    }
    let Some(source) = chunk.source.as_ref() else {
        return false;
    };
    if !source.contains_key("source_ms") {
        return false;
    }
    let live_text = source
        .get("text")
        .and_then(Value::as_str)
        .map(str::trim)
        .unwrap_or_default();
    if live_text.is_empty() {
        return false;
    }
    let rest = strip_leading_bracket_clock(expected_indexed_content);
    let label = source.get("speaker").and_then(Value::as_str).unwrap_or("");
    if label.is_empty() {
        rest == live_text
    } else {
        rest == format!("{label}: {live_text}")
    }
}

struct CachedSpeechLine {
    indexed_content: String,
    spoken_body: String,
    raw_line: String,
    statement_ordinal: i64,
}

enum CachedSpeechFile {
    Unreadable,
    Granola {
        untimed: bool,
        chunks: Vec<solstone_core_format::content::IndexChunk>,
    },
    Speech {
        untimed: bool,
        metadata: Option<serde_json::Map<String, Value>>,
        labels: Option<BTreeMap<i64, ResolvedSpeakerLabel>>,
        run_numbers: Option<BTreeMap<i64, i64>>,
        lines: Vec<CachedSpeechLine>,
    },
}

/// Per-response cache for live verification and speaker display of indexed speech lines.
#[derive(Default)]
pub struct SpeechLineCache {
    files: BTreeMap<String, CachedSpeechFile>,
}

impl SpeechLineCache {
    /// Resolve one indexed speech chunk against the live file on disk.
    ///
    /// Returns `None` if the segment or file is missing, unreadable, outside the
    /// journal, or no live line preserves the indexed spoken words (fail-closed
    /// on deletion, redaction, or text edits). Timestamp or speaker-label drift
    /// with unchanged spoken words resolves to the live disk line.
    pub fn resolve_line(
        &mut self,
        journal_root: &Path,
        rel: &str,
        chunk_idx: i64,
        expected_indexed_content: &str,
        speakers: Option<&AgentSpeakers>,
        possible_names: Option<&PossibleNameLabels>,
    ) -> Option<ResolvedSpeechLine> {
        let idx = usize::try_from(chunk_idx).ok()?;
        let normalized = rel.strip_prefix("chronicle/").unwrap_or(rel);
        let filename = normalized.rsplit('/').next()?;
        let cached = self
            .files
            .entry(normalized.to_owned())
            .or_insert_with(|| load_cached_speech_file(journal_root, normalized, speakers));
        match cached {
            CachedSpeechFile::Unreadable => None,
            CachedSpeechFile::Granola { untimed, chunks } => {
                let chunk = chunks
                    .get(idx)
                    .filter(|c| granola_chunk_matches_expected(c, expected_indexed_content))
                    .or_else(|| {
                        chunks
                            .iter()
                            .enumerate()
                            .filter(|(_, c)| {
                                c.source
                                    .as_ref()
                                    .is_some_and(|s| s.contains_key("source_ms"))
                                    && granola_chunk_matches_expected(c, expected_indexed_content)
                            })
                            .min_by_key(|(i, _)| i.abs_diff(idx))
                            .map(|(_, c)| c)
                    })?;
                let source = chunk.source.as_ref();
                let is_spoken = source.is_some_and(|record| record.contains_key("source_ms"));
                let time = (!*untimed && is_spoken)
                    .then(|| {
                        source
                            .and_then(|record| record.get("start"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    })
                    .flatten();
                let speaker = is_spoken
                    .then(|| {
                        source
                            .and_then(|record| record.get("speaker"))
                            .and_then(Value::as_str)
                            .filter(|s| !s.is_empty())
                            .map(str::to_owned)
                    })
                    .flatten();
                Some(ResolvedSpeechLine {
                    text: chunk.content.clone(),
                    time,
                    speaker,
                    is_owner: false,
                    speaker_entity_id: None,
                    untimed: *untimed,
                })
            }
            CachedSpeechFile::Speech {
                untimed,
                metadata,
                labels,
                run_numbers,
                lines,
            } => {
                let line = lines
                    .get(idx)
                    .filter(|l| {
                        l.indexed_content.trim() == expected_indexed_content.trim()
                            || speech_chunk_matches_body(expected_indexed_content, &l.spoken_body)
                    })
                    .or_else(|| {
                        lines
                            .iter()
                            .enumerate()
                            .filter(|(_, l)| {
                                l.indexed_content.trim() == expected_indexed_content.trim()
                                    || speech_chunk_matches_body(
                                        expected_indexed_content,
                                        &l.spoken_body,
                                    )
                            })
                            .min_by_key(|(i, _)| i.abs_diff(idx))
                            .map(|(_, l)| l)
                    })?;
                let name_labels = possible_names.filter(|labels| labels.file == filename);
                render_speech_record(
                    normalized,
                    *untimed,
                    metadata.as_ref(),
                    labels.as_ref(),
                    run_numbers.as_ref(),
                    name_labels,
                    &line.raw_line,
                    line.statement_ordinal,
                )
            }
        }
    }
}

fn load_cached_speech_file(
    journal_root: &Path,
    normalized_rel: &str,
    speakers: Option<&AgentSpeakers>,
) -> CachedSpeechFile {
    let parts: Vec<&str> = normalized_rel.split('/').collect();
    let [day, stream, seg_key, filename] = parts.as_slice() else {
        return CachedSpeechFile::Unreadable;
    };
    let Ok(Some(segment)) = Segment::resolve_exact(journal_root, day, stream, seg_key) else {
        return CachedSpeechFile::Unreadable;
    };
    let file_path = segment.path().join(filename);
    let Ok(text) = read_bounded_utf8_path(&file_path) else {
        return CachedSpeechFile::Unreadable;
    };
    let produced = produce_chunks(Family::Audio, normalized_rel, &text);
    let parsed_lines = parse_jsonl_objects_with_raw(&text);
    let metadata = parsed_lines
        .first()
        .filter(|(_, record)| !record.contains_key("start"))
        .map(|(_, record)| record.clone());
    let untimed = metadata
        .as_ref()
        .is_some_and(|meta| meta.get("untimed") == Some(&Value::Bool(true)));
    if metadata.as_ref().is_some_and(|meta| {
        meta.get(solstone_core_format::content::granola::HEADER_KEY)
            .is_some_and(Value::is_object)
    }) {
        return CachedSpeechFile::Granola {
            untimed,
            chunks: produced.chunks,
        };
    }
    let labels = speakers.and_then(|speakers| {
        let (source, names) = local_speaker_names(segment.path(), speakers)?;
        (filename.strip_suffix(".jsonl") == Some(source.as_str())).then_some(names)
    });
    let run_numbers = speakers.and_then(|_| {
        let stem = filename.strip_suffix(".jsonl")?;
        crate::run_numbers_by_source(segment.path(), std::slice::from_ref(&file_path)).remove(stem)
    });
    let mut lines = Vec::with_capacity(produced.chunks.len());
    let mut statement_ordinal = 0_u64;
    let mut chunk_iter = produced.chunks.into_iter().peekable();
    for (index, (raw, record)) in parsed_lines.into_iter().enumerate() {
        if index == 0 && metadata.is_some() {
            continue;
        }
        let Some(ordinal) = next_statement_ordinal(&raw, &mut statement_ordinal) else {
            continue;
        };
        if let Some(chunk) = chunk_iter.peek()
            && chunk.source.as_ref() == Some(&record)
        {
            let chunk = chunk_iter.next().expect("peeked chunk");
            let spoken_body = crate::statement_text(&record)
                .map(|(_, body)| body.trim().to_owned())
                .unwrap_or_default();
            lines.push(CachedSpeechLine {
                indexed_content: chunk.content,
                spoken_body,
                raw_line: raw,
                statement_ordinal: ordinal,
            });
        }
    }
    CachedSpeechFile::Speech {
        untimed,
        metadata,
        labels,
        run_numbers,
        lines,
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::{
        MAX_TRANSCRIPT_PAGE_BYTES, MAX_TRANSCRIPT_PAGE_ITEMS, read_segment_transcript_page,
        read_segment_transcript_page_leaving_out, segment_clock,
    };
    use solstone_core_format::content::{
        Family, RawPerceptFamily, produce_chunks, produce_raw_percept_chunks,
    };
    use solstone_core_journal_io::paths::{PathOrDay, iter_segments};

    #[test]
    fn oversized_chat_round_trips_across_utf8_and_escaped_boundaries() {
        use solstone_core_format::content::AdmittedCategory::{Messages, Transcripts};
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("chronicle/20260914/import.claude/090000_300");
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("conversation_transcript.jsonl");
        let words = "first paragraph\n\n`quoted` \"hello\" \\ café 🦊".repeat(4000);
        let escaped=serde_json::json!({"start":"00:00:00","text":words,"private_field":"unrelated metadata"}).to_string().replace("🦊","\\ud83e\\udd8a");
        fs::write(
            &file,
            format!("{{}}\n{escaped}\n{{\"start\":\"00:00:01\",\"text\":\"following turn\"}}\n"),
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let read = |cursor: Option<&super::SegmentTranscriptCursor>, permission| {
            super::read_segment_transcript_page_with_permissions(
                &segment,
                cursor,
                None,
                None,
                &BTreeSet::new(),
                Some(&BTreeSet::from([permission])),
            )
        };
        let mut cursor = None;
        let mut pieces = Vec::new();
        let mut saved = None;
        loop {
            let page = read(cursor.as_ref(), Messages).unwrap();
            pieces.extend(page.entries.into_iter().map(|e| e.text));
            cursor = page.next;
            if saved.is_none() {
                saved = cursor.clone()
            }
            if cursor.is_none() {
                break;
            }
        }
        assert_eq!(pieces.pop().as_deref(), Some("following turn"));
        assert!(pieces.len() > 10);
        assert_eq!(pieces.concat(), words);
        assert!(read(saved.as_ref(), Transcripts).is_err());
        // A same-size edit can preserve mtime. Native identity and change
        // stamp still prevent the next page from using the original bounds.
        let modified = fs::metadata(&file).unwrap().modified().unwrap();
        let original = fs::read_to_string(&file).unwrap();
        fs::write(&file, original.replace("hello", "world")).unwrap();
        fs::File::options()
            .write(true)
            .open(&file)
            .unwrap()
            .set_modified(modified)
            .unwrap();
        assert!(read(saved.as_ref(), Messages).is_err());
        fs::write(&file, b"{}\n").unwrap();
        assert!(read(saved.as_ref(), Messages).is_err());
    }

    #[test]
    fn long_turn_rechecks_its_opened_medium_and_bounds_display_metadata() {
        use super::long_message;
        use solstone_core_format::content::conversation::ConversationMedium::{Spoken, Written};
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("chronicle/20260914/import.text/090000_300");
        fs::create_dir_all(&dir).unwrap();
        let name = "conversation_transcript.jsonl";
        let path = dir.join(name);
        let origin = "20260914/import.text/090000_300";
        let relative = format!("{origin}/{name}");
        let header = |reason| {
            serde_json::json!({"imported":{"id":"fixture","triage":{"kind":"transcripts","ruleset_version":7,"reasons":[reason]}}}).to_string()
        };
        let row = serde_json::json!({"start":"00:01:02","speaker":"A".repeat(90000),"text":"body","partial_input":{"dropped_entries":3}});
        let spoken = header("webvtt_cues");
        fs::write(&path, format!("{spoken}\n{row}\n")).unwrap();
        assert!(long_message::bound_read(&path, &relative, Written, |_, _| Ok(())).is_err());
        assert!(long_message::bound_read(&path, &relative, Spoken, |_, _| Ok(())).is_ok());
        let written = header("zoom_chat");
        fs::write(&path, format!("{written}\n{row}\n")).unwrap();
        let result = long_message::bound_read(&path, &relative, Written, |file, version| {
            long_message::passage(
                file,
                version,
                written.len() as u64 + 1,
                None,
                name,
                origin,
                false,
            )
        })
        .unwrap();
        assert_eq!(result.text, "[00:01:02] body");
        assert!(
            result
                .part
                .unwrap()
                .source_notice
                .unwrap()
                .contains("dropped_entries: 3")
        );
    }

    #[test]
    fn each_conversation_and_its_neighbors_use_their_own_grants() {
        use serde_json::json;
        use solstone_core_format::content::AdmittedCategory::{Messages, Notes, Transcripts};

        for (stream, name, reason, grants) in [
            (
                "import.chatgpt",
                "conversation_transcript.jsonl",
                None,
                vec![Messages],
            ),
            (
                "import.claude",
                "imported_audio.jsonl",
                None,
                vec![Messages],
            ),
            (
                "import.gemini",
                "conversation_transcript.jsonl",
                None,
                vec![Messages],
            ),
            (
                "import.text",
                "conversation_transcript.jsonl",
                Some("zoom_chat"),
                vec![Messages],
            ),
            (
                "import.text",
                "conversation_transcript.jsonl",
                Some("webvtt_cues"),
                vec![Transcripts],
            ),
            (
                "import.text",
                "conversation_transcript.jsonl",
                Some("bracketed_speaker"),
                vec![Messages, Transcripts],
            ),
            (
                "import.text",
                "conversation_transcript.jsonl",
                None,
                vec![Messages, Transcripts],
            ),
        ] {
            let root = tempfile::tempdir().unwrap();
            let dir = root
                .path()
                .join("chronicle/20260914")
                .join(stream)
                .join("090000_300");
            fs::create_dir_all(&dir).unwrap();
            let mut header = json!({"imported":{"id":"fixture"}});
            if let Some(reason) = reason {
                header["imported"]["triage"] =
                    json!({"kind":"transcripts","ruleset_version":7,"reasons":[reason]});
            }
            fs::write(
                dir.join(name),
                format!(
                    "{header}\n{}\n",
                    json!({"start":"00:00:00","text":"turn-marker"})
                ),
            )
            .unwrap();
            fs::write(dir.join("note_transcript.md"), "neighbor-marker\n").unwrap();
            let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
                .unwrap()
                .pop()
                .unwrap();
            for categories in [
                vec![],
                vec![Messages],
                vec![Transcripts],
                vec![Notes],
                vec![Messages, Transcripts],
                vec![Messages, Notes, Transcripts],
            ] {
                let categories = categories.into_iter().collect::<BTreeSet<_>>();
                let page = super::read_segment_transcript_page_with_permissions(
                    &segment,
                    None,
                    None,
                    None,
                    &BTreeSet::new(),
                    Some(&categories),
                );
                let conversation = grants.iter().all(|grant| categories.contains(grant));
                let note = categories.contains(&Notes);
                assert_eq!(
                    super::segment_has_permitted_files(&segment, &categories),
                    conversation || note
                );
                if conversation || note {
                    let entries = page.unwrap().entries;
                    assert_eq!(
                        entries
                            .iter()
                            .any(|entry| entry.text.contains("turn-marker")),
                        conversation
                    );
                    assert_eq!(
                        entries
                            .iter()
                            .any(|entry| entry.text.contains("neighbor-marker")),
                        note
                    );
                } else {
                    assert!(matches!(
                        page,
                        Err(super::SegmentTranscriptReadError::NoPermittedFiles)
                    ));
                }
            }
        }
    }

    #[test]
    fn changing_a_sidecar_relabels_only_admitted_conversation_pages() {
        use solstone_core_format::content::{
            AdmittedCategory::{Messages, Notes, Transcripts},
            IMPORT_KIND_FILENAME,
        };
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("chronicle/20260914/import.text/090000_300");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("note_transcript.md"), "neighbor").unwrap();
        let turn = serde_json::json!({"start":"00:00:01","text":"a turn"}).to_string();
        fs::write(
            dir.join("conversation_transcript.jsonl"),
            format!(
                "{{\"imported\":{{\"id\":\"fixture\"}}}}\n{}",
                format!("{turn}\n").repeat(MAX_TRANSCRIPT_PAGE_ITEMS + 1)
            ),
        )
        .unwrap();
        let mark = |reason| {
            fs::write(dir.join(IMPORT_KIND_FILENAME), serde_json::json!({"import_id":"fixture","kind":"transcripts","ruleset_version":7,"reasons":[reason]}).to_string()).unwrap()
        };
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let read = |permissions: &BTreeSet<_>, cursor: Option<&super::SegmentTranscriptCursor>| {
            super::read_segment_transcript_page_with_permissions(
                &segment,
                cursor,
                None,
                None,
                &BTreeSet::new(),
                Some(permissions),
            )
        };
        let both = BTreeSet::from([Messages, Transcripts]);
        let notes = BTreeSet::from([Notes]);
        mark("zoom_chat");
        let messages = read(&both, None).unwrap();
        assert_eq!(messages.content_kinds, ["messages"]);
        let neighbor = read(&notes, None).unwrap();
        mark("webvtt_cues");
        assert_eq!(neighbor, read(&notes, None).unwrap());
        assert!(messages.next.is_some());
        assert!(matches!(
            read(&both, messages.next.as_ref()),
            Err(super::SegmentTranscriptReadError::Changed)
        ));
        let speech = read(&both, None).unwrap();
        assert_eq!(speech.content_kinds, ["transcripts"]);
    }

    #[test]
    fn hidden_voice_edits_do_not_invalidate_a_notes_only_page() {
        use solstone_core_format::content::AdmittedCategory::Notes;
        let root = tempfile::tempdir().unwrap();
        let dir = named_segment(root.path());
        fs::write(dir.join("note_transcript.md"), "A note").unwrap();
        fs::write(
            dir.join("audio.jsonl"),
            r#"{"start":"00:00:01","speaker":1,"text":"Speech"}"#,
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let read = || {
            super::read_segment_transcript_page_with_permissions(
                &segment,
                None,
                Some(&speakers()),
                None,
                &BTreeSet::new(),
                Some(&BTreeSet::from([Notes])),
            )
            .unwrap()
        };
        let original = read();
        fs::write(
            dir.join("talents/speaker_labels.json"),
            r#"{"labels":[{"sentence_id":1,"speaker":"person-a","method":"acoustic"}]}"#,
        )
        .unwrap();
        assert_eq!(read(), original);
        assert!(
            original
                .entries
                .iter()
                .all(|entry| !entry.text.contains("Speech"))
        );
    }

    #[test]
    fn routine_tests_for_segment_clock() {
        let clock = segment_clock("000000_86400").unwrap();
        assert_eq!(clock.label, "00:00-23:59");
        assert_eq!(clock.start_seconds, 0);
        assert_eq!(clock.exclusive_end_seconds, 86400);

        let clock = segment_clock("110000_60").unwrap();
        assert_eq!(clock.label, "11:00-11:01");
        assert_eq!(clock.start_seconds, 39600);
        assert_eq!(clock.exclusive_end_seconds, 39660);

        let clock = segment_clock("110000_30").unwrap();
        assert_eq!(clock.label, "11:00-11:00");
        assert_eq!(clock.start_seconds, 39600);
        assert_eq!(clock.exclusive_end_seconds, 39630);

        let clock = segment_clock("233000_3600").unwrap();
        assert_eq!(clock.label, "23:30-23:59");
        assert_eq!(clock.start_seconds, 84600);
        assert_eq!(clock.exclusive_end_seconds, 86400);

        let clock = segment_clock("x-120000_300").unwrap();
        assert_eq!(clock.label, "12:00-12:05");
        assert_eq!(clock.start_seconds, 43200);
        assert_eq!(clock.exclusive_end_seconds, 43500);
    }

    #[test]
    fn pages_one_large_transcript_without_reading_talent_or_percept_files() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("meeting_transcript.md"),
            "approved transcript line\n".repeat(MAX_TRANSCRIPT_PAGE_BYTES / 8),
        )
        .unwrap();
        fs::write(segment.join("screen.jsonl"), "secret percept").unwrap();
        fs::write(segment.join("talents/brief.md"), "secret talent").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let first = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert!(!first.entries.is_empty());
        assert!(
            first
                .entries
                .iter()
                .all(|entry| entry.text == "approved transcript line")
        );
        assert!(first.next.is_some());
        assert!(!first.untimed);
    }

    #[test]
    fn transcript_page_reads_speech_recorded_under_any_name() {
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle/20261023/device_audio/140000_12");
        fs::create_dir_all(&segment).unwrap();
        fs::write(segment.join("note-0.wav"), b"RIFF").unwrap();
        fs::write(
            segment.join("note-0.jsonl"),
            concat!(
                r#"{"raw":"note-0.wav","_solstone_processing":{"schema":"solstone.processing.v1","state":"analyzed","handler":"transcribe","input_size":4}}"#,
                "\n",
                r#"{"start":"14:00:00","text":"Maya checked the orchard irrigation pump this morning."}"#,
                "\n",
            ),
        )
        .unwrap();
        fs::write(
            segment.join("events.jsonl"),
            "{\"start\":\"14:00:01\",\"text\":\"ingest bookkeeping\"}\n",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20261023"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert_eq!(
            page.entries,
            vec![super::SegmentTranscriptEntry {
                turn_part: None,
                text: "[14:00:00] Maya checked the orchard irrigation pump this morning."
                    .to_owned(),
                channel: "Transcript",
            }]
        );
    }

    #[test]
    fn transcript_page_projects_recorded_speech_without_other_sources() {
        let root = tempfile::tempdir().unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        fs::write(
            segment.join("meeting_transcript.md"),
            "approved transcript line\n",
        )
        .unwrap();
        fs::write(
            segment.join("audio.jsonl"),
            "{\"recording\":\"metadata only\"}\n{\"start\":\"00:00:01\",\"text\":\"what was said\",\"raw\":\"secret audio metadata\"}\n",
        )
        .unwrap();
        fs::write(segment.join("screen.jsonl"), "screen percept\n").unwrap();
        fs::write(segment.join("browser_history.jsonl"), "browser percept\n").unwrap();
        fs::write(segment.join("talents/brief.md"), "secret talent\n").unwrap();
        fs::write(segment.join("talents/sense.json"), "secret sense\n").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert_eq!(
            page.entries,
            vec![
                super::SegmentTranscriptEntry {
                    turn_part: None,
                    text: "[00:00:01] what was said".to_owned(),
                    channel: "Transcript",
                },
                super::SegmentTranscriptEntry {
                    turn_part: None,
                    text: "approved transcript line".to_owned(),
                    channel: "Imported Conversation",
                },
            ]
        );
        assert!(!page.untimed);
    }

    #[test]
    fn audio_and_aichat_projections_and_renderer_guard() {
        let root = tempfile::tempdir().unwrap();
        // 1. import.audio
        let seg_audio = root
            .path()
            .join("chronicle/20260914/import.audio/090000_300");
        fs::create_dir_all(&seg_audio).unwrap();
        let audio_bytes =
            "{\"recording\":\"metadata\"}\n{\"start\":\"00:00:02\",\"text\":\"spoken words\"}\n";
        fs::write(seg_audio.join("imported_audio.jsonl"), audio_bytes).unwrap();

        // 2. legacy import.chatgpt
        let seg_chatgpt = root
            .path()
            .join("chronicle/20260914/import.chatgpt/090000_300");
        fs::create_dir_all(&seg_chatgpt).unwrap();
        let chat_bytes = "{\"imported\":{}}\n{\"start\":\"00:00:00\",\"speaker\":\"Ana Lima\",\"text\":\"Hello all.\"}\n";
        fs::write(seg_chatgpt.join("imported_audio.jsonl"), chat_bytes).unwrap();

        // 3. source_audio.jsonl in default
        let seg_custom = root.path().join("chronicle/20260914/default/090500_300");
        fs::create_dir_all(&seg_custom).unwrap();
        fs::write(seg_custom.join("mic_audio.jsonl"), audio_bytes).unwrap();

        // 4. timed import.text
        let seg_text = root
            .path()
            .join("chronicle/20260914/import.text/091000_300");
        fs::create_dir_all(&seg_text).unwrap();
        fs::write(seg_text.join("conversation_transcript.jsonl"), chat_bytes).unwrap();

        let segments = iter_segments(root.path(), PathOrDay::Day("20260914")).unwrap();

        // Verify import.audio page
        let seg = segments.iter().find(|s| s.path() == seg_audio).unwrap();
        let page = read_segment_transcript_page(seg, None, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:02] spoken words");

        // Verify custom _audio.jsonl page
        let seg = segments.iter().find(|s| s.path() == seg_custom).unwrap();
        let page = read_segment_transcript_page(seg, None, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:02] spoken words");

        // Verify import.chatgpt page
        let seg = segments.iter().find(|s| s.path() == seg_chatgpt).unwrap();
        let page = read_segment_transcript_page(seg, None, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:00] **Ana Lima:** Hello all.");

        // Verify import.text page
        let seg = segments.iter().find(|s| s.path() == seg_text).unwrap();
        let page = read_segment_transcript_page(seg, None, None, None).unwrap();
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].text, "[00:00:00] **Ana Lima:** Hello all.");

        // Renderer guard: call produce_raw_percept_chunks and produce_chunks directly
        let raw = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            "20260914/import.audio/090000_300/imported_audio.jsonl",
            audio_bytes,
        );
        assert_eq!(raw.chunks.len(), 1);
        assert_eq!(raw.chunks[0].content, "[00:00:02] spoken words");

        let chat = produce_chunks(
            Family::AiChat,
            "20260914/import.chatgpt/090000_300/imported_audio.jsonl",
            chat_bytes,
        );
        assert_eq!(chat.chunks.len(), 1);
        assert_eq!(chat.chunks[0].content, "**Ana Lima:** Hello all.");
        assert_eq!(chat.header.as_deref(), Some("# ChatGPT conversation"));
    }

    #[test]
    fn shared_thread_time_is_omitted_and_a_sourced_midnight_is_kept() {
        use serde_json::{Value, json};
        for (stream, sent_at, timed) in [
            ("import.claude", None, false),
            ("import.claude", Some(Value::Null), false),
            ("import.gemini", None, false),
            ("import.gemini", Some(Value::Null), false),
            ("import.chatgpt", Some(json!("2026-09-14T00:00:00Z")), true),
            ("import.claude", Some(json!("2026-09-14T00:00:00Z")), true),
        ] {
            let mut row = json!({"start":"00:00:00","speaker":"Author","text":"An exchange"});
            if let Some(sent_at) = sent_at {
                row["sent_at"] = sent_at;
            }
            let origin = format!("20260914/{stream}/000000_300");
            let text = super::project_line(
                &origin,
                "conversation_transcript.jsonl",
                false,
                false,
                &row.to_string(),
                None,
            );
            assert_eq!(text.starts_with("[00:00:00]"), timed, "{stream}: {row}");
            assert!(text.ends_with("An exchange"));
        }
    }

    #[test]
    fn a_flat_legacy_source_transcript_reads_projected_not_raw() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let segment = root.path().join("chronicle/20250301/093000_300");
        fs::create_dir_all(&segment).unwrap();
        fs::write(
            segment.join("session_transcript.jsonl"),
            "{\"raw\": \"session.flac\"}\n{\"start\": \"00:00:04\", \"speaker\": 2, \"text\": \"send the deck on friday\"}\n",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20250301"))
            .unwrap()
            .pop()
            .unwrap();
        let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert_eq!(page.entries.len(), 1, "{:?}", page.entries);
        assert!(page.entries[0].text.contains("send the deck on friday"));
        assert!(
            !page.entries[0].text.contains('{'),
            "{}",
            page.entries[0].text
        );
    }

    #[test]
    fn untimed_flag_persists_across_continuation_pages() {
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle/20260914/import.text/090000_300");
        fs::create_dir_all(&segment).unwrap();

        let mut lines = String::from("{\"untimed\": true, \"imported\": {}}\n");
        for i in 1..=MAX_TRANSCRIPT_PAGE_ITEMS + 10 {
            lines.push_str(&format!(
                "{{\"start\": \"00:00:00\", \"speaker\": \"User\", \"text\": \"turn {i}\"}}\n"
            ));
        }
        fs::write(segment.join("conversation_transcript.jsonl"), lines).unwrap();

        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let first = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert!(first.untimed);
        assert_eq!(first.entries.len(), MAX_TRANSCRIPT_PAGE_ITEMS);
        assert_eq!(first.entries[0].channel, "Imported Conversation");
        assert_eq!(first.entries[0].text, "**User:** turn 1");
        assert!(first.next.is_some());

        let second =
            read_segment_transcript_page(&segment, first.next.as_ref(), None, None).unwrap();
        assert!(second.untimed);
        assert_eq!(second.entries.len(), 10);
        assert_eq!(second.entries[0].text, "**User:** turn 101");
        assert!(second.next.is_none());
    }

    #[test]
    fn a_strava_workout_reads_as_what_strava_recorded_on_its_first_piece_only() {
        let root = tempfile::tempdir().unwrap();
        let workout = serde_json::json!({
            "schema": "solstone.import.strava.tile.v1",
            "activity_id": 9,
            "tile": {"index": 0, "count": 2},
            "workout": {"name": "Morning Run", "type": "Run", "distance_m": 10200.0}
        });
        for (key, index) in [("070200_300", 0), ("070700_300", 1)] {
            let segment = root
                .path()
                .join("chronicle/20261004/import.strava")
                .join(key);
            fs::create_dir_all(&segment).unwrap();
            let mut tile = workout.clone();
            tile["tile"]["index"] = serde_json::json!(index);
            fs::write(segment.join("workout.json"), format!("{tile}\n")).unwrap();
        }
        let segments = iter_segments(root.path(), PathOrDay::Day("20261004")).unwrap();
        let pages = segments
            .iter()
            .map(|segment| {
                read_segment_transcript_page(segment, None, None, None)
                    .unwrap()
                    .entries
            })
            .collect::<Vec<_>>();
        assert_eq!(
            pages[0],
            vec![super::SegmentTranscriptEntry {
                turn_part: None,
                text: "## Strava workout: Morning Run\nRun\n10.2 km".to_owned(),
                channel: "Imported Text",
            }]
        );
        assert!(pages[1].is_empty());
    }

    #[test]
    fn a_body_window_leaves_out_the_rows_asked_and_its_version_says_so() {
        use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};
        let root = tempfile::tempdir().unwrap();
        let segment = root
            .path()
            .join("chronicle/20261104/import.apple_health/010000_3600");
        fs::create_dir_all(&segment).unwrap();
        let row = |id: &str| {
            let mut row = BodyRow::new(
                RowKind::Reading,
                id.to_owned(),
                "apple_health",
                "HKQuantityTypeIdentifierHeartRate",
                "heart rate".into(),
            );
            row.at = Some("2026-11-04 01:10:00 -0700".into());
            row.value = Some(format!("{}", id.len() + 60));
            row
        };
        fs::write(
            segment.join("body.jsonl"),
            encode_rows(&[row("kept"), row("gone")]),
        )
        .unwrap();
        let segments = iter_segments(root.path(), PathOrDay::Day("20261104")).unwrap();
        let plain = read_segment_transcript_page(&segments[0], None, None, None).unwrap();
        assert_eq!(plain.entries.len(), 2);
        let none = read_segment_transcript_page_leaving_out(
            &segments[0],
            None,
            None,
            None,
            &BTreeSet::new(),
        )
        .unwrap();
        assert_eq!(none, plain);
        let left = read_segment_transcript_page_leaving_out(
            &segments[0],
            None,
            None,
            None,
            &BTreeSet::from(["gone".to_owned()]),
        )
        .unwrap();
        assert_eq!(left.entries, plain.entries[..1]);
        assert_ne!(left.version, plain.version);
    }

    #[test]
    fn item_limited_page_resumes_at_the_first_unreturned_line() {
        let root = tempfile::TempDir::new_in("/var/tmp").unwrap();
        let segment = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(&segment).unwrap();
        let lines = (1..=MAX_TRANSCRIPT_PAGE_ITEMS + 5)
            .map(|line| format!("line {line}\n"))
            .collect::<String>();
        fs::write(segment.join("meeting_transcript.md"), lines).unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let first = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert_eq!(first.entries.len(), MAX_TRANSCRIPT_PAGE_ITEMS);
        assert_eq!(first.entries[0].text, "line 1");
        assert_eq!(
            first.entries[MAX_TRANSCRIPT_PAGE_ITEMS - 1].text,
            "line 100"
        );

        let second =
            read_segment_transcript_page(&segment, first.next.as_ref(), None, None).unwrap();
        assert_eq!(
            second
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            ["line 101", "line 102", "line 103", "line 104", "line 105"]
        );
        assert!(second.next.is_none());
    }

    fn speakers() -> super::AgentSpeakers {
        super::AgentSpeakers {
            voices: crate::VoiceNames {
                principal: "person-b".into(),
                owner_voice_confirmed: true,
                people: [("person-a".to_owned(), "Ada".to_owned())].into(),
                owner_name: None,
            },
            owner: "Bea (owner)".into(),
        }
    }

    fn named_segment(root: &std::path::Path) -> std::path::PathBuf {
        let segment = root.join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(segment.join("talents")).unwrap();
        for (id, entity) in [
            (
                "person-a",
                serde_json::json!({"name":"Ada", "type":"Person"}),
            ),
            (
                "person-b",
                serde_json::json!({"name":"Bea", "type":"Person", "is_principal":true}),
            ),
            (
                "tool",
                serde_json::json!({"name":"Secret tool", "type":"Tool"}),
            ),
            (
                "blocked",
                serde_json::json!({"name":"Blocked name", "type":"Person", "blocked":true}),
            ),
            ("nameless", serde_json::json!({"type":"Person"})),
        ] {
            let dir = root.join("entities").join(id);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("entity.json"), entity.to_string()).unwrap();
        }
        segment
    }

    #[test]
    fn local_names_follow_statement_ordinals_across_pages() {
        let root = tempfile::tempdir().unwrap();
        let dir = named_segment(root.path());
        let mut text = String::from("{\"raw\":\"audio.flac\"}\n\ninvalid\n");
        for id in 1..=102 {
            // Explicit IDs deliberately disagree with the owner-view ordinals.
            text.push_str(
                &serde_json::json!({"start":"00:00:01", "sentence_id":9000+id,
                "source":"mic", "speaker":1, "text":format!("sentence {id}"),
                "ignored": "x".repeat(1024)})
                .to_string(),
            );
            text.push('\n');
        }
        fs::write(dir.join("audio.jsonl"), text).unwrap();
        fs::write(
            dir.join("talents/speaker_labels.json"),
            serde_json::json!({"labels":[
                {"sentence_id":1,"speaker":"person-b","method":"owner_centroid"},
                {"sentence_id":100,"speaker":"person-a","method":"acoustic"},
                {"sentence_id":101,"speaker":"person-b","method":"owner_centroid","owner_margin_declined":true},
                {"sentence_id":102,"speaker":"person-a","method":"contextual"}
            ]})
            .to_string(),
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let first = read_segment_transcript_page(&segment, None, Some(&speakers()), None).unwrap();
        assert_eq!(
            first.entries[0].text,
            "[00:00:01] (mic) Bea (owner): sentence 1"
        );
        assert_eq!(first.entries[99].text, "[00:00:01] (mic) Ada: sentence 100");
        let second =
            read_segment_transcript_page(&segment, first.next.as_ref(), Some(&speakers()), None)
                .unwrap();
        assert_eq!(
            second
                .entries
                .iter()
                .map(|row| row.text.as_str())
                .collect::<Vec<_>>(),
            // A declined owner match and a name inferred from what was said name no one.
            [
                "[00:00:01] (mic) Speaker 1: sentence 101",
                "[00:00:01] (mic) Speaker 1: sentence 102"
            ]
        );
        assert!(second.next.is_none());

        // The rendered identity is part of the cursor version.
        fs::write(
            dir.join("talents/speaker_labels.json"),
            serde_json::json!({"labels":[
                {"sentence_id":1,"speaker":"person-b","method":"owner_centroid"},
                {"sentence_id":100,"speaker":"person-a","method":"contextual"}
            ]})
            .to_string(),
        )
        .unwrap();
        assert!(matches!(
            read_segment_transcript_page(&segment, first.next.as_ref(), Some(&speakers()), None),
            Err(super::SegmentTranscriptReadError::Changed)
        ));
    }

    #[test]
    fn label_admission_and_source_selection_preserve_unnamed_text() {
        let root = tempfile::tempdir().unwrap();
        let dir = named_segment(root.path());
        let text = (1..=6)
            .map(|id| {
                format!("{{\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"sentence {id}\"}}\n")
            })
            .collect::<String>();
        fs::write(dir.join("audio.jsonl"), &text).unwrap();
        let rows = ["person-a", "tool", "blocked", "missing", "nameless"]
            .iter()
            .enumerate()
            .map(|(i, id)| serde_json::json!({"sentence_id":i+1,"speaker":id,"method":"acoustic"}))
            .collect::<Vec<_>>();
        let labels = serde_json::json!({"labels":rows}).to_string();
        fs::write(dir.join("talents/speaker_labels.json"), &labels).unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let read =
            || read_segment_transcript_page(&segment, None, Some(&speakers()), None).unwrap();
        let named = read();
        assert_eq!(named.entries[0].text, "[00:00:01] Ada: sentence 1");
        for id in 2..=6 {
            assert_eq!(
                named.entries[id - 1].text,
                format!("[00:00:01] Speaker 1: sentence {id}")
            );
        }
        // Two sources and no embedding source are ambiguous, so neither is named.
        fs::write(dir.join("mic_audio.jsonl"), &text).unwrap();
        assert!(
            read()
                .entries
                .iter()
                .all(|row| row.text.contains("Speaker 1:"))
        );
        // The owner's source rule chooses the npz stem; no names bleed into audio.
        fs::write(dir.join("mic_audio.npz"), "").unwrap();
        let selected = read();
        assert_eq!(selected.entries[0].text, "[00:00:01] Speaker 1: sentence 1");
        assert_eq!(selected.entries[6].text, "[00:00:01] Ada: sentence 1");
        for bytes in ["{".to_owned(), "x".repeat(MAX_TRANSCRIPT_PAGE_BYTES + 1)] {
            fs::write(dir.join("talents/speaker_labels.json"), bytes).unwrap();
            assert!(
                read()
                    .entries
                    .iter()
                    .all(|row| row.text.contains("Speaker 1:"))
            );
        }
        fs::remove_file(dir.join("talents/speaker_labels.json")).unwrap();
        assert!(
            read()
                .entries
                .iter()
                .all(|row| row.text.contains("Speaker 1:"))
        );
        // Without agent speakers, every line keeps its stored label.
        fs::write(dir.join("talents/speaker_labels.json"), labels).unwrap();
        assert!(
            read_segment_transcript_page(&segment, None, None, None)
                .unwrap()
                .entries
                .iter()
                .all(|row| row.text.contains("Speaker 1:"))
        );
    }

    /// One segment's speech: `(diarization number, voice, text)` per line, with
    /// each voice a distinct direction in the 256-wide embedding.
    fn audio_with_voices(dir: &std::path::Path, lines: &[(i64, usize, &str)]) {
        use std::io::Write;
        let mut text = String::from("{\"raw\":\"audio.flac\"}\n");
        let mut payload = Vec::new();
        let mut ids = Vec::new();
        for (index, (speaker, voice, line)) in lines.iter().enumerate() {
            text.push_str(
                &serde_json::json!({"start":"00:00:01","source":"mic","speaker":speaker,"text":line})
                    .to_string(),
            );
            text.push('\n');
            for column in 0..256 {
                let value: f32 = if column == *voice { 1.0 } else { 0.0 };
                payload.extend_from_slice(&value.to_le_bytes());
            }
            ids.extend_from_slice(&(index as i32 + 1).to_le_bytes());
        }
        fs::write(dir.join("audio.jsonl"), text).unwrap();
        let mut zip = zip::ZipWriter::new(fs::File::create(dir.join("audio.npz")).unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        let rows = lines.len();
        zip.start_file("embeddings.npy", options).unwrap();
        zip.write_all(&solstone_core_npy::write_npy(
            "<f4",
            &format!("({rows}, 256)"),
            &payload,
        ))
        .unwrap();
        zip.start_file("statement_ids.npy", options).unwrap();
        zip.write_all(&solstone_core_npy::write_npy(
            "<i4",
            &format!("({rows},)"),
            &ids,
        ))
        .unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn an_agent_read_keeps_one_number_on_one_voice_through_a_recording() {
        let root = tempfile::tempdir().unwrap();
        let first = named_segment(root.path());
        let second = first.with_file_name("090500_300");
        fs::create_dir_all(&second).unwrap();
        // Diarization numbers the two voices one way, then the other.
        audio_with_voices(&first, &[(1, 0, "song"), (2, 1, "talk")]);
        audio_with_voices(&second, &[(2, 0, "song again"), (1, 1, "talk again")]);
        let segments = iter_segments(root.path(), PathOrDay::Day("20260914")).unwrap();
        let read = |key: &str, speakers: Option<&super::AgentSpeakers>| {
            let segment = segments
                .iter()
                .find(|segment| segment.key() == key)
                .unwrap();
            read_segment_transcript_page(segment, None, speakers, None)
                .unwrap()
                .entries
                .into_iter()
                .map(|entry| entry.text)
                .collect::<Vec<_>>()
        };
        let agent = speakers();
        let later = read("090500_300", Some(&agent));
        assert!(later[0].contains("Speaker 1: song again"), "{later:?}");
        assert!(later[1].contains("Speaker 2: talk again"), "{later:?}");
        // A read without speaker names keeps the stored diarization numbers.
        let stored = read("090500_300", None);
        assert!(stored[0].contains("Speaker 2: song again"), "{stored:?}");
    }

    #[test]
    fn audio_named_chat_keeps_its_chat_speakers_even_with_local_labels() {
        let root = tempfile::tempdir().unwrap();
        let old_dir = named_segment(root.path());
        let dir = root
            .path()
            .join("chronicle/20260914/import.chatgpt/090000_300");
        fs::create_dir_all(dir.parent().unwrap()).unwrap();
        fs::rename(old_dir, &dir).unwrap();
        fs::write(
            dir.join("imported_audio.jsonl"),
            "{\"start\":\"00:00:00\",\"speaker\":\"User\",\"text\":\"hello\"}\n",
        )
        .unwrap();
        fs::write(
            dir.join("talents/speaker_labels.json"),
            "{\"labels\":[{\"sentence_id\":1,\"speaker\":\"person-a\",\"method\":\"acoustic\"}]}",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let page = read_segment_transcript_page(&segment, None, Some(&speakers()), None).unwrap();
        assert_eq!(page.entries[0].text, "[00:00:00] **User:** hello");
        assert!(crate::labelled_audio_source(&dir).is_none());
        assert!(crate::speaker_label_source(&dir).0.is_none());
    }

    #[test]
    fn audio_transcript_page_appends_warning_to_flagged_line_only() {
        let root = tempfile::tempdir().unwrap();
        let segment_dir = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(&segment_dir).unwrap();

        let flagged_line = serde_json::json!({
            "start": "00:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
            "turn_boundary_suspect": true,
            "speaker_turn_evidence": {
                "version": "window-local-turns-v1",
                "audio_duration_s": 20.0,
                "ranges": [{"start_s": 11.0, "end_s": 12.0}],
            },
        });
        let neighbor_line = serde_json::json!({
            "start": "00:00:15",
            "text": "Neighbor speech without warning.",
            "speaker": 2,
            "sentence_id": 2,
        });

        let audio_content = format!(
            "{}\n{}\n",
            serde_json::to_string(&flagged_line).unwrap(),
            serde_json::to_string(&neighbor_line).unwrap()
        );
        fs::write(segment_dir.join("audio.jsonl"), audio_content).unwrap();

        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();

        let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
        assert_eq!(page.entries.len(), 2);

        let flagged_entry = &page.entries[0].text;
        assert!(flagged_entry.contains("Is the gate open? It is open."));
        assert!(flagged_entry.contains("Speaker 1:"));
        assert!(flagged_entry.contains(super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE));
        assert!(flagged_entry.ends_with(&format!(
            "[{}]",
            super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE
        )));

        let neighbor_entry = &page.entries[1].text;
        let neighbor_chunks = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            "chronicle/20260914/default/090000_300/audio.jsonl",
            &serde_json::to_string(&neighbor_line).unwrap(),
        );
        assert_eq!(neighbor_entry, &neighbor_chunks.chunks[0].content);
        assert!(!neighbor_entry.contains(super::POSSIBLE_SPEAKER_CHANGE_WITHIN_SENTENCE));
    }

    #[test]
    fn produce_raw_percept_chunks_ignores_turn_boundary_suspect_and_speaker_turn_evidence() {
        let flagged = serde_json::json!({
            "start": "00:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
            "turn_boundary_suspect": true,
            "speaker_turn_evidence": {
                "version": "window-local-turns-v1",
                "audio_duration_s": 20.0,
                "ranges": [{"start_s": 11.0, "end_s": 12.0}],
            },
        });
        let unflagged = serde_json::json!({
            "start": "00:00:10",
            "text": "Is the gate open? It is open.",
            "speaker": 1,
            "sentence_id": 1,
        });

        let rel = "chronicle/20260914/default/090000_300/audio.jsonl";
        let chunks_flagged = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            rel,
            &serde_json::to_string(&flagged).unwrap(),
        );
        let chunks_unflagged = produce_raw_percept_chunks(
            RawPerceptFamily::Audio,
            rel,
            &serde_json::to_string(&unflagged).unwrap(),
        );

        assert_eq!(chunks_flagged.chunks.len(), chunks_unflagged.chunks.len());
        assert_eq!(
            chunks_flagged.chunks[0].content,
            chunks_unflagged.chunks[0].content
        );
        assert_eq!(
            chunks_flagged.chunks[0].occurrence_time_ms,
            chunks_unflagged.chunks[0].occurrence_time_ms
        );
    }

    fn possible_names(file: &str, rows: &[(i64, usize, &str)]) -> crate::PossibleNameLabels {
        let mut labels = crate::PossibleNameLabels {
            file: file.to_owned(),
            ..Default::default()
        };
        for (sentence, end, name) in rows {
            labels
                .statements
                .entry(*sentence)
                .or_default()
                .push((*end, (*name).to_owned()));
        }
        labels
    }

    #[test]
    fn possible_names_follow_their_words_and_bind_the_page_version() {
        let root = tempfile::tempdir().unwrap();
        let dir = root.path().join("chronicle/20260914/default/090000_300");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("audio.jsonl"),
            "{\"raw\":\"audio.flac\"}\n\
             {\"start\":\"00:00:01\",\"speaker\":1,\"text\":\"ask Fryer to call anna\"}\n\
             {\"start\":\"00:00:02\",\"speaker\":2,\"text\":\"heard wrong\",\"corrected\":\"ask Fryer again\"}\n",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let plain = read_segment_transcript_page(&segment, None, None, None).unwrap();
        let labels = possible_names(
            "audio.jsonl",
            &[(1, 22, "Ana Lima"), (1, 9, "Friar"), (2, 9, "Friar")],
        );
        let labelled = read_segment_transcript_page(&segment, None, None, Some(&labels)).unwrap();
        assert_eq!(
            labelled
                .entries
                .iter()
                .map(|entry| entry.text.as_str())
                .collect::<Vec<_>>(),
            [
                "[00:00:01] Speaker 1: ask Fryer (possible name: Friar) to call anna (possible name: Ana Lima)",
                "[00:00:02] Speaker 2: ask Fryer (possible name: Friar) again",
            ]
        );
        assert_ne!(plain.version, labelled.version);

        // No label to show reads exactly as no labels at all.
        let empty = possible_names("audio.jsonl", &[]);
        let unlabelled = read_segment_transcript_page(&segment, None, None, Some(&empty)).unwrap();
        assert_eq!(unlabelled, plain);

        // Labels for another file show nothing but still bind the version.
        let other = possible_names("mic_audio.jsonl", &[(1, 9, "Friar")]);
        let elsewhere = read_segment_transcript_page(&segment, None, None, Some(&other)).unwrap();
        assert_eq!(elsewhere.entries, plain.entries);

        // A continuation minted before the labels changed is refused.
        let changed = possible_names("audio.jsonl", &[(1, 9, "Friar Tuck")]);
        let cursor = super::SegmentTranscriptCursor {
            source_index: 0,
            byte_offset: 0,
            statement_ordinal: 0,
            message: None,
            version: labelled.version.clone(),
        };
        assert!(matches!(
            read_segment_transcript_page(&segment, Some(&cursor), None, Some(&changed)),
            Err(super::SegmentTranscriptReadError::Changed)
        ));
    }

    #[test]
    fn possible_names_never_label_imported_chat() {
        let root = tempfile::tempdir().unwrap();
        let dir = root
            .path()
            .join("chronicle/20260914/import.chatgpt/090000_300");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("imported_audio.jsonl"),
            "{\"start\":\"00:00:00\",\"speaker\":\"User\",\"text\":\"hello Fryer\"}\n",
        )
        .unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260914"))
            .unwrap()
            .pop()
            .unwrap();
        let labels = possible_names("imported_audio.jsonl", &[(1, 11, "Friar")]);
        let page = read_segment_transcript_page(&segment, None, None, Some(&labels)).unwrap();
        assert_eq!(page.entries[0].text, "[00:00:00] **User:** hello Fryer");
    }

    #[test]
    fn imported_channels_follow_their_kind_and_content_kinds_respect_permissions() {
        use super::read_segment_transcript_page_with_permissions;
        use solstone_core_format::content::AdmittedCategory;

        let root = tempfile::tempdir().unwrap();
        for (seg_key, stream, file, body, expected_channel, expected_kind) in [
            (
                "090000_300",
                "import.text",
                "note_transcript.md",
                "field note\n",
                "Note",
                "notes",
            ),
            (
                "090500_300",
                "import.text",
                "document_transcript.md",
                "manual text\n",
                "Document",
                "documents",
            ),
            (
                "091000_300",
                "import.document",
                "document_transcript.md",
                "pdf manual\n",
                "Document",
                "documents",
            ),
            (
                "091500_300",
                "import.text",
                "conversation_transcript.jsonl",
                "{\"imported\":{}}\n{\"start\":\"00:00:00\",\"speaker\":\"Ana\",\"text\":\"hello\"}\n",
                "Imported Conversation",
                "",
            ),
            (
                "092000_300",
                "import.text",
                "imported.md",
                "pre-triage text\n",
                "Imported Conversation",
                "",
            ),
            (
                "092500_300",
                "import.image",
                "screenshot_transcript.md",
                "receipt text\n",
                "Screenshot",
                "screen_text",
            ),
            (
                "093000_300",
                "import.image",
                "image_transcript.md",
                "garden bench\n",
                "Photo",
                "photos",
            ),
            (
                "093500_300",
                "import.obsidian",
                "note_transcript.md",
                "vault note\n",
                "Note",
                "notes",
            ),
            (
                "094000_300",
                "import.evernote",
                "note_transcript.md",
                "evernote note\n",
                "Note",
                "notes",
            ),
            (
                "094500_300",
                "import.kindle",
                "book_transcript.md",
                "highlight line\n",
                "Highlights",
                "notes",
            ),
            (
                "095000_300",
                "import.ics",
                "event_transcript.md",
                "event summary\n",
                "Calendar Event",
                "calendar",
            ),
        ] {
            let dir = root
                .path()
                .join(format!("chronicle/20260407/{stream}/{seg_key}"));
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(file), body).unwrap();
            let segment = iter_segments(root.path(), PathOrDay::Day("20260407"))
                .unwrap()
                .into_iter()
                .find(|seg| seg.path() == dir)
                .unwrap();
            let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
            assert_eq!(page.entries[0].channel, expected_channel, "{stream}/{file}");
            assert_eq!(page.unresolved_conversation, expected_kind.is_empty());
            assert_eq!(
                page.content_kinds,
                if expected_kind.is_empty() {
                    vec![]
                } else {
                    vec![expected_kind]
                },
                "{stream}/{file}"
            );
        }

        // Mixed segment: a transcripts-only connection sees only transcripts, never notes.
        let mixed = root.path().join("chronicle/20260407/mixed/100000_300");
        fs::create_dir_all(&mixed).unwrap();
        fs::write(
            mixed.join("audio.jsonl"),
            "{\"source\":\"recording\"}\n{\"start\":\"10:00:01\",\"text\":\"spoken line\"}\n",
        )
        .unwrap();
        fs::write(mixed.join("note_transcript.md"), "private note\n").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260407"))
            .unwrap()
            .into_iter()
            .find(|seg| seg.path() == mixed)
            .unwrap();

        let tx_only = BTreeSet::from([AdmittedCategory::Transcripts]);
        let page = read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&tx_only),
        )
        .unwrap();
        assert_eq!(page.content_kinds, vec!["transcripts"]);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].channel, "Transcript");

        let both = BTreeSet::from([AdmittedCategory::Transcripts, AdmittedCategory::Notes]);
        let page = read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&both),
        )
        .unwrap();
        assert_eq!(page.content_kinds, vec!["transcripts", "notes"]);
        assert_eq!(page.entries.len(), 2);

        // Screen-only segment: reports screen_text only when screenshots is held.
        let screen = root
            .path()
            .join("chronicle/20260407/macos_screen/110000_300");
        fs::create_dir_all(screen.join("talents")).unwrap();
        fs::write(screen.join("talents/screen_text.md"), "editor text\n").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260407"))
            .unwrap()
            .into_iter()
            .find(|seg| seg.path() == screen)
            .unwrap();
        let with_screen = BTreeSet::from([
            AdmittedCategory::Transcripts,
            AdmittedCategory::ScreenText,
            AdmittedCategory::Screenshots,
        ]);
        let page = read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&with_screen),
        )
        .unwrap();
        assert_eq!(page.content_kinds, vec!["screen_text"]);
        let without_screen = read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&tx_only),
        )
        .unwrap();
        assert!(without_screen.content_kinds.is_empty());
    }

    #[test]
    fn entries_carry_each_files_registry_content_kind() {
        use solstone_core_format::content::body_records::{BodyRow, RowKind, encode_rows};

        let root = tempfile::tempdir().unwrap();
        let mixed = root
            .path()
            .join("chronicle/20260407/import.obsidian/090000_300");
        fs::create_dir_all(&mixed).unwrap();
        // Both files get channel "Note" on import.obsidian, yet carry distinct
        // registry kinds (documents vs notes).
        fs::write(mixed.join("document_transcript.md"), "manual line\n").unwrap();
        fs::write(mixed.join("note_transcript.md"), "### Photo\nfield note\n").unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260407"))
            .unwrap()
            .into_iter()
            .find(|seg| seg.path() == mixed)
            .unwrap();
        let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
        let paired: Vec<(&str, Option<&str>, &str)> = page
            .entries_with_content_kind()
            .map(|(entry, kind)| (entry.channel, kind, entry.text.as_str()))
            .collect();
        assert_eq!(
            paired,
            vec![
                ("Note", Some("documents"), "manual line"),
                ("Note", Some("notes"), "### Photo"),
                ("Note", Some("notes"), "field note"),
            ]
        );

        // Body-only segment carries channel "Body Records" and content_kind "body".
        let body_seg = root
            .path()
            .join("chronicle/20260407/import.apple_health/070000_3600");
        fs::create_dir_all(&body_seg).unwrap();
        let mut row = BodyRow::new(
            RowKind::Reading,
            "hr1".to_owned(),
            "apple_health",
            "HKQuantityTypeIdentifierHeartRate",
            "heart rate".to_owned(),
        );
        row.at = Some("2026-04-07 07:15:00 -0400".to_owned());
        row.value = Some("64 bpm".to_owned());
        fs::write(body_seg.join("body.jsonl"), encode_rows(&[row])).unwrap();
        let segment = iter_segments(root.path(), PathOrDay::Day("20260407"))
            .unwrap()
            .into_iter()
            .find(|seg| seg.path() == body_seg)
            .unwrap();
        let page = read_segment_transcript_page(&segment, None, None, None).unwrap();
        let paired: Vec<(&str, Option<&str>)> = page
            .entries_with_content_kind()
            .map(|(entry, kind)| (entry.channel, kind))
            .collect();
        assert_eq!(paired, vec![("Body Records", Some("body"))]);
    }

    #[test]
    fn selected_email_fixture_respects_messages_and_documents_grants() {
        use solstone_core_format::content::AdmittedCategory;

        let root = tempfile::tempdir().unwrap();
        let seg = root
            .path()
            .join("chronicle/20261118/import.email/101600_300");
        fs::create_dir_all(&seg).unwrap();
        let header = serde_json::json!({
            "source": "email",
            "provider": "gmail",
            "first_sent_at": "2026-11-18T17:16:00Z",
            "last_sent_at": "2026-11-18T17:16:00Z",
            "email_schema": 1
        });
        let row = serde_json::json!({
            "start": "10:16:00",
            "source_ms": 1795022160000_i64,
            "sent_at": "2026-11-18T10:16:00-07:00",
            "speaker": "Robin",
            "is_owner": true,
            "to": ["Casey"],
            "cc": ["Lee"],
            "text": "Yes, the afternoon works.\n\nI've attached the revised agenda.",
            "reply_to": {
                "author": "Casey",
                "sent_at": "2026-11-17T16:20:00-07:00",
                "text": "Could we meet on Thursday afternoon?",
                "truncated": true
            },
            "attachments": [
                { "name": "agenda.pdf", "item_ref": "fixture-document-01" },
                { "name": "site-photo.jpg" }
            ],
            "email": { "provider": "gmail", "key": "gm:1" }
        });
        fs::write(
            seg.join("conversation_transcript.jsonl"),
            format!("{header}\n{row}\n"),
        )
        .unwrap();
        fs::write(
            seg.join("document_transcript.md"),
            "Plan B is in room seven.\n",
        )
        .unwrap();

        let segment = iter_segments(root.path(), PathOrDay::Day("20261118"))
            .unwrap()
            .into_iter()
            .find(|s| s.path() == seg)
            .unwrap();

        // Messages-only reader sees the exchange and both filenames, never the extracted PDF.
        let messages_only = BTreeSet::from([AdmittedCategory::Messages]);
        let page = super::read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&messages_only),
        )
        .unwrap();
        assert_eq!(page.content_kinds, vec!["messages"]);
        assert_eq!(page.entries.len(), 1);
        assert_eq!(page.entries[0].channel, "Email");
        let text = &page.entries[0].text;
        assert!(text.contains("Yes, the afternoon works."), "{text}");
        assert!(
            text.contains("Could we meet on Thursday afternoon?"),
            "{text}"
        );
        assert!(text.contains("agenda.pdf"), "{text}");
        assert!(text.contains("site-photo.jpg"), "{text}");
        assert!(!text.contains("Plan B is in room seven."), "{text}");

        // Documents-only reader sees the extracted PDF, never the email exchange.
        let docs_only = BTreeSet::from([AdmittedCategory::Documents]);
        let doc_page = super::read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&docs_only),
        )
        .unwrap();
        assert_eq!(doc_page.content_kinds, vec!["documents"]);
        assert!(
            doc_page
                .entries
                .iter()
                .any(|e| e.text.contains("Plan B is in room seven."))
        );
        assert!(
            !doc_page
                .entries
                .iter()
                .any(|e| e.text.contains("afternoon works"))
        );

        // Transcripts-only (spoken) reader sees neither.
        let transcripts_only = BTreeSet::from([AdmittedCategory::Transcripts]);
        let err = super::read_segment_transcript_page_with_permissions(
            &segment,
            None,
            None,
            None,
            &BTreeSet::new(),
            Some(&transcripts_only),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            crate::SegmentTranscriptReadError::NoPermittedFiles
        ));
    }

    #[test]
    fn resolve_line_recovers_timestamp_and_index_drift_but_fails_closed_on_text_edits() {
        use std::collections::BTreeMap;

        let root = tempfile::tempdir().unwrap();
        let seg = root.path().join("chronicle/20261119/talk/193000_720");
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(
            seg.join("audio.jsonl"),
            concat!(
                "{\"raw\":\"audio.flac\"}\n",
                "{\"sentence_id\":1,\"speaker\":\"other_person\",\"start\":\"19:30:05\",\"text\":\"Will you bring the ledger?\"}\n",
                "{\"sentence_id\":2,\"speaker\":\"owner_id\",\"start\":\"19:30:52\",\"text\":\"Yes. I will bring twenty pounds before noon.\"}\n",
                "{\"sentence_id\":3,\"speaker\":\"(the templar)\",\"start\":\"19:31:10\",\"text\":\"He said: meet at the bridge.\"}\n",
            ),
        )
        .unwrap();
        fs::write(
            seg.join("talents/speaker_labels.json"),
            serde_json::json!({
                "labels": [
                    {"sentence_id": 1, "speaker": "person_b", "method": "user_assigned", "confidence": "high"},
                    {"sentence_id": 2, "speaker": "person_owner", "method": "user_assigned", "confidence": "high"}
                ]
            })
            .to_string(),
        )
        .unwrap();

        let speakers = super::AgentSpeakers {
            owner: "Alex Rivera (owner)".to_owned(),
            voices: crate::VoiceNames {
                principal: "person_owner".to_owned(),
                owner_voice_confirmed: true,
                people: BTreeMap::from([("person_b".to_owned(), "Sam Patel".to_owned())]),
                owner_name: Some("Alex Rivera (owner)".to_owned()),
            },
        };
        let rel = "20261119/talk/193000_720/audio.jsonl";
        let mut cache = super::SpeechLineCache::default();

        // Stale clock at same index (19:30:48 -> live 19:30:52) recovers live clock and voice attribution.
        let drifted = cache
            .resolve_line(
                root.path(),
                rel,
                1,
                "[19:30:48] owner_id: Yes. I will bring twenty pounds before noon.",
                Some(&speakers),
                None,
            )
            .expect("timestamp drift should resolve to live disk line");
        assert_eq!(drifted.time.as_deref(), Some("19:30:52"));
        assert_eq!(drifted.speaker.as_deref(), Some("Alex Rivera (owner)"));
        assert!(drifted.is_owner);
        assert_eq!(drifted.speaker_entity_id.as_deref(), Some("person_owner"));
        assert!(drifted.text.starts_with("[19:30:52] Alex Rivera (owner):"));

        // Shifted index with unnamed parenthetical speaker recovers live line without claiming ownership.
        let shifted = cache
            .resolve_line(
                root.path(),
                rel,
                0,
                "[19:31:00] (the templar): He said: meet at the bridge.",
                Some(&speakers),
                None,
            )
            .expect("shifted index with unchanged spoken words should resolve");
        assert_eq!(shifted.time.as_deref(), Some("19:31:10"));
        assert_eq!(shifted.speaker.as_deref(), Some("(the templar)"));
        assert!(!shifted.is_owner);
        assert_eq!(shifted.speaker_entity_id, None);

        // Redacted or edited spoken words fail closed.
        assert!(
            cache
                .resolve_line(
                    root.path(),
                    rel,
                    1,
                    "[19:30:52] owner_id: Yes. I will bring thirty pounds before noon.",
                    Some(&speakers),
                    None,
                )
                .is_none()
        );
        // Truncating after an embedded colon in the spoken text must also fail closed.
        assert!(
            cache
                .resolve_line(
                    root.path(),
                    rel,
                    2,
                    "[19:31:10] (the templar): Different prefix: He said: meet at the bridge.",
                    Some(&speakers),
                    None,
                )
                .is_none()
        );

        // Deleting the file fails closed on a fresh cache.
        fs::remove_file(seg.join("audio.jsonl")).unwrap();
        let mut fresh = super::SpeechLineCache::default();
        assert!(
            fresh
                .resolve_line(
                    root.path(),
                    rel,
                    1,
                    "[19:30:52] owner_id: Yes. I will bring twenty pounds before noon.",
                    Some(&speakers),
                    None,
                )
                .is_none()
        );
    }

    #[test]
    fn read_spoken_segment_lines_filters_non_speech_and_respects_owner_clears() {
        use solstone_core_format::content::AdmittedCategory::{Messages, Notes, Transcripts};
        use std::collections::BTreeMap;

        let root = tempfile::tempdir().unwrap();
        let seg = root.path().join("chronicle/20261119/talk/090000_300");
        fs::create_dir_all(seg.join("talents")).unwrap();
        fs::write(
            seg.join("audio.jsonl"),
            concat!(
                "{\"raw\":\"audio.flac\"}\n",
                "{\"sentence_id\":1,\"speaker\":1,\"start\":\"09:00:05\",\"text\":\"Owner confirmed line.\"}\n",
                "{\"sentence_id\":2,\"speaker\":2,\"start\":\"09:00:15\",\"text\":\"Cleared owner line.\"}\n",
                "{\"sentence_id\":3,\"speaker\":3,\"start\":\"09:00:25\",\"text\":\"Unnamed speaker line.\"}\n",
            ),
        )
        .unwrap();
        // Sibling non-spoken note should never appear in spoken-only reads.
        fs::write(seg.join("note_transcript.md"), "Private note text\n").unwrap();
        fs::write(
            seg.join("talents/speaker_labels.json"),
            serde_json::json!({
                "labels": [
                    {"sentence_id": 1, "speaker": "person_owner", "method": "user_assigned", "confidence": "high"},
                    {"sentence_id": 2, "speaker": "person_owner", "method": "owner_centroid", "confidence": "high"},
                    {"sentence_id": 3, "speaker": "person_other", "method": "text_inferred", "confidence": "low"}
                ]
            })
            .to_string(),
        )
        .unwrap();
        // Clear sentence 2 via owner_line_labels.json so it must stay anonymous and not is_owner.
        let cleared_sha = {
            use sha2::{Digest, Sha256};
            format!("{:x}", Sha256::digest(b"Cleared owner line."))
        };
        fs::write(
            seg.join("talents/owner_line_labels.json"),
            serde_json::json!({
                "version": 1,
                "entries": [{
                    "source": "audio",
                    "sentence_id": 2,
                    "start": "09:00:15",
                    "text_sha256": cleared_sha,
                    "speaker": null,
                    "set_at": "2026-11-19T09:05:00Z"
                }]
            })
            .to_string(),
        )
        .unwrap();

        let segment = iter_segments(root.path(), PathOrDay::Day("20261119"))
            .unwrap()
            .pop()
            .unwrap();
        let speakers = super::AgentSpeakers {
            owner: "Alex Rivera (owner)".to_owned(),
            voices: crate::VoiceNames {
                principal: "person_owner".to_owned(),
                owner_voice_confirmed: true,
                people: BTreeMap::from([("person_other".to_owned(), "Sam Patel".to_owned())]),
                owner_name: Some("Alex Rivera (owner)".to_owned()),
            },
        };

        // Without Transcripts permission, spoken read fails closed even when Notes/Messages are granted.
        let no_transcripts = BTreeSet::from([Notes, Messages]);
        assert!(!super::segment_has_spoken_transcript_files(
            &segment,
            &no_transcripts
        ));
        assert!(matches!(
            super::read_spoken_segment_lines(
                &segment,
                Some(&speakers),
                None,
                Some(&no_transcripts),
            ),
            Err(crate::SegmentTranscriptReadError::NoPermittedFiles)
        ));

        let grants = BTreeSet::from([Transcripts, Notes]);
        assert!(super::segment_has_spoken_transcript_files(
            &segment, &grants
        ));
        let spoken =
            super::read_spoken_segment_lines(&segment, Some(&speakers), None, Some(&grants))
                .expect("spoken lines");
        assert_eq!(spoken.lines.len(), 3);
        assert!(
            spoken
                .lines
                .iter()
                .all(|l| !l.text.contains("Private note"))
        );

        // Line 1: voice-attributed owner.
        assert!(spoken.lines[0].is_owner);
        assert_eq!(
            spoken.lines[0].speaker.as_deref(),
            Some("Alex Rivera (owner)")
        );
        assert_eq!(
            spoken.lines[0].speaker_entity_id.as_deref(),
            Some("person_owner")
        );

        // Line 2: cleared by owner_line_labels.json -> NOT owner, no entity id, plain speaker number.
        assert!(!spoken.lines[1].is_owner);
        assert_eq!(spoken.lines[1].speaker_entity_id, None);
        assert_eq!(spoken.lines[1].speaker.as_deref(), Some("Speaker 2"));

        // Line 3: text_inferred is ignored by voice gate -> plain speaker number.
        assert!(!spoken.lines[2].is_owner);
        assert_eq!(spoken.lines[2].speaker_entity_id, None);
        assert_eq!(spoken.lines[2].speaker.as_deref(), Some("Speaker 3"));

        assert_eq!(
            spoken.speakers,
            vec!["Alex Rivera (owner)", "Speaker 2", "Speaker 3"]
        );
    }
}
