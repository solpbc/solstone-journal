// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cmp::Reverse;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

use serde::Deserialize;
use serde_json::{Map, Value, json};
use solstone_core_format::content::body_records::BODY_RECORDS_FILE;
use solstone_core_indexer_query::{AdmittedCategory, ConnectionScope};
use solstone_core_indexer_store::classification::{FacetDeclarationSet, classify_source};
use solstone_core_journal_io::paths::{
    PathOrDay, day_dirs, iter_segments_skipping_unreadable_streams,
};
use solstone_core_transcripts::{permitted_segment_files_for_summary, segment_clock};

use super::{
    MAX_DAY_BYTES, MAX_FACET_BYTES, MAX_OPAQUE_REFERENCE_BYTES, ToolError,
    optional_string_within_limit, search::MAX_LIMIT,
};
use crate::dispatch::{
    DispatchError, Prepared, boundary, segment_assignment_probe_path, segment_boundary,
    source_label, source_labels,
};
use crate::permissions::ConnectionReadSnapshot;

pub(crate) struct ValidatedListTranscripts {
    pub(crate) day: Option<String>,
    pub(crate) day_from: Option<String>,
    pub(crate) day_to: Option<String>,
    pub(crate) facet_id: Option<String>,
    pub(crate) limit: usize,
    pub(crate) cursor: Option<String>,
    pub(crate) time_from: Option<String>,
    pub(crate) time_to: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) kind: Option<String>,
    pub(crate) speaker: Option<String>,
    pub(crate) summary: bool,
}

use solstone_core_observe_audio::{MAX_CLIP_DURATION_MS, MIN_CLIP_DURATION_MS};

pub(crate) const DEFAULT_AUDIO_DURATION_MS: u64 = 30_000;
pub(crate) const MAX_AUDIO_START_MS: u64 = 86_400_000;
pub(crate) const MAX_CONTEXT_LINES: usize = 5;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AudioEncoding {
    Content,
    Base64,
}

impl AudioEncoding {
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Content => "content",
            Self::Base64 => "base64",
        }
    }
}

pub(crate) struct AudioClipRequest {
    pub(crate) start_ms: u64,
    pub(crate) duration_ms: u64,
    pub(crate) track: Option<String>,
    pub(crate) encoding: AudioEncoding,
}

pub(crate) struct TranscriptRangeRequest {
    pub(crate) day: Option<String>,
    pub(crate) day_from: Option<String>,
    pub(crate) day_to: Option<String>,
    pub(crate) facet_id: Option<String>,
    pub(crate) source: Option<String>,
    pub(crate) speaker: Option<String>,
    pub(crate) context_lines: usize,
    pub(crate) cursor: Option<String>,
}

pub(crate) struct TranscriptReferenceRequest {
    pub(crate) offset_ms: Option<u32>,
    pub(crate) display: Option<String>,
    pub(crate) audio: Option<AudioClipRequest>,
    pub(crate) reference: String,
    pub(crate) speaker: Option<String>,
    pub(crate) context_lines: usize,
    pub(crate) cursor: Option<String>,
}

pub(crate) enum ValidatedGetTranscript {
    Reference(TranscriptReferenceRequest),
    Range(TranscriptRangeRequest),
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ListParams {
    #[serde(default)]
    day: Option<String>,
    #[serde(default, alias = "from")]
    day_from: Option<String>,
    #[serde(default, alias = "to")]
    day_to: Option<String>,
    #[serde(default)]
    facet: Option<String>,
    #[serde(default)]
    limit: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
    #[serde(default)]
    time_from: Option<String>,
    #[serde(default)]
    time_to: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default, alias = "kind")]
    entry_kind: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    summary: bool,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct GetParams {
    offset_ms: Option<u32>,
    display: Option<String>,
    #[serde(default)]
    audio: Option<bool>,
    #[serde(default)]
    audio_start_ms: Option<u64>,
    #[serde(default)]
    audio_duration_ms: Option<u64>,
    #[serde(default)]
    audio_track: Option<String>,
    #[serde(default)]
    audio_encoding: Option<String>,
    #[serde(default)]
    reference: Option<String>,
    #[serde(default)]
    day: Option<String>,
    #[serde(default, alias = "from")]
    day_from: Option<String>,
    #[serde(default, alias = "to")]
    day_to: Option<String>,
    #[serde(default)]
    facet: Option<String>,
    #[serde(default)]
    source: Option<String>,
    #[serde(default)]
    speaker: Option<String>,
    #[serde(default)]
    context_lines: Option<usize>,
    #[serde(default)]
    cursor: Option<String>,
}

const fn default_limit() -> usize {
    10
}

fn validate_time(time_str: &str) -> Result<(), ToolError> {
    if time_str.len() != 5 {
        return Err(ToolError::InvalidInput);
    }
    let bytes = time_str.as_bytes();
    if !bytes[0].is_ascii_digit()
        || !bytes[1].is_ascii_digit()
        || bytes[2] != b':'
        || !bytes[3].is_ascii_digit()
        || !bytes[4].is_ascii_digit()
    {
        return Err(ToolError::InvalidInput);
    }
    let hour = time_str[0..2]
        .parse::<u32>()
        .map_err(|_| ToolError::InvalidInput)?;
    let minute = time_str[3..5]
        .parse::<u32>()
        .map_err(|_| ToolError::InvalidInput)?;
    if hour > 23 || minute > 59 {
        return Err(ToolError::InvalidInput);
    }
    Ok(())
}

pub(crate) fn validate_list(params: Option<&Value>) -> Result<ValidatedListTranscripts, ToolError> {
    let params = params.cloned().unwrap_or_else(|| serde_json::json!({}));
    let params =
        serde_json::from_value::<ListParams>(params).map_err(|_| ToolError::InvalidInput)?;
    if params.summary
        && (params.limit.is_some()
            || params.cursor.is_some()
            || params.time_from.is_some()
            || params.time_to.is_some()
            || params.source.is_some()
            || params.entry_kind.is_some()
            || params.speaker.is_some())
    {
        return Err(ToolError::InvalidInput);
    }
    let limit = params.limit.unwrap_or_else(default_limit);
    validate_limit(limit)?;
    if !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
        || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
        || !optional_string_within_limit(&params.speaker, MAX_OPAQUE_REFERENCE_BYTES)
        || params
            .speaker
            .as_deref()
            .is_some_and(|word| word.trim().is_empty())
    {
        return Err(ToolError::InvalidInput);
    }
    if !optional_string_within_limit(&params.source, MAX_FACET_BYTES)
        || !optional_string_within_limit(&params.entry_kind, MAX_FACET_BYTES)
        || params
            .entry_kind
            .as_deref()
            .is_some_and(|kind| kind.trim().is_empty())
    {
        return Err(ToolError::InvalidInput);
    }
    let normalized_day = super::normalize_day(params.day)?;
    let day_from = super::normalize_day(params.day_from)?;
    let day_to = super::normalize_day(params.day_to)?;
    if (normalized_day.is_some() && (day_from.is_some() || day_to.is_some()))
        || matches!((&day_from, &day_to), (Some(from), Some(to)) if to < from)
    {
        return Err(ToolError::ActivityDayArguments);
    }
    if params.entry_kind.is_some()
        && (params.source.is_none()
            || normalized_day.is_none()
            || params.facet.is_some()
            || params.speaker.is_some())
    {
        return Err(ToolError::SourceArguments);
    }
    if (params.time_from.is_some() || params.time_to.is_some()) && normalized_day.is_none() {
        return Err(ToolError::TranscriptTimeArguments);
    }
    if let Some(ref tf) = params.time_from {
        validate_time(tf)?;
    }
    if let Some(ref tt) = params.time_to {
        validate_time(tt)?;
    }
    if matches!((&params.time_from, &params.time_to), (Some(tf), Some(tt)) if tf > tt) {
        return Err(ToolError::TranscriptTimeArguments);
    }
    if params
        .cursor
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }
    Ok(ValidatedListTranscripts {
        day: normalized_day,
        day_from,
        day_to,
        facet_id: params.facet,
        limit,
        cursor: params.cursor,
        time_from: params.time_from,
        time_to: params.time_to,
        source: params.source,
        kind: params.entry_kind,
        speaker: params.speaker,
        summary: params.summary,
    })
}

pub(crate) fn validate_get(params: Option<&Value>) -> Result<ValidatedGetTranscript, ToolError> {
    let params = params.cloned().ok_or(ToolError::InvalidInput)?;
    let params =
        serde_json::from_value::<GetParams>(params).map_err(|_| ToolError::InvalidInput)?;
    validate_limit(1)?;
    if params
        .cursor
        .as_ref()
        .is_some_and(|value| value.is_empty() || value.len() > MAX_OPAQUE_REFERENCE_BYTES)
    {
        return Err(ToolError::InvalidInput);
    }
    if !optional_string_within_limit(&params.speaker, MAX_OPAQUE_REFERENCE_BYTES)
        || params
            .speaker
            .as_deref()
            .is_some_and(|word| word.trim().is_empty())
    {
        return Err(ToolError::InvalidInput);
    }
    if let Some(ctx) = params.context_lines
        && (ctx > MAX_CONTEXT_LINES || params.speaker.is_none())
    {
        return Err(ToolError::InvalidInput);
    }
    let context_lines = params.context_lines.unwrap_or(0);

    let has_range_fields = params.day.is_some()
        || params.day_from.is_some()
        || params.day_to.is_some()
        || params.facet.is_some()
        || params.source.is_some();
    let audio_options_present = params.audio_start_ms.is_some()
        || params.audio_duration_ms.is_some()
        || params.audio_track.is_some()
        || params.audio_encoding.is_some();

    if has_range_fields {
        if params.reference.is_some()
            || params.offset_ms.is_some()
            || params.display.is_some()
            || params.audio.is_some()
            || audio_options_present
            || !optional_string_within_limit(&params.day, MAX_DAY_BYTES)
            || !optional_string_within_limit(&params.day_from, MAX_DAY_BYTES)
            || !optional_string_within_limit(&params.day_to, MAX_DAY_BYTES)
            || !optional_string_within_limit(&params.facet, MAX_FACET_BYTES)
            || !optional_string_within_limit(&params.source, MAX_FACET_BYTES)
            || params
                .source
                .as_deref()
                .is_some_and(|source| source.trim().is_empty())
        {
            return Err(ToolError::InvalidInput);
        }
        let day = super::normalize_day(params.day)?;
        let day_from = super::normalize_day(params.day_from)?;
        let day_to = super::normalize_day(params.day_to)?;
        if day.is_none() && day_from.is_none() && day_to.is_none() {
            return Err(ToolError::InvalidInput);
        }
        if (day.is_some() && (day_from.is_some() || day_to.is_some()))
            || matches!((&day_from, &day_to), (Some(from), Some(to)) if to < from)
        {
            return Err(ToolError::ActivityDayArguments);
        }
        return Ok(ValidatedGetTranscript::Range(TranscriptRangeRequest {
            day,
            day_from,
            day_to,
            facet_id: params.facet,
            source: params.source,
            speaker: params.speaker,
            context_lines,
            cursor: params.cursor,
        }));
    }

    let Some(reference) = params.reference else {
        return Err(ToolError::InvalidInput);
    };
    if reference.is_empty() || reference.len() > MAX_OPAQUE_REFERENCE_BYTES {
        return Err(ToolError::InvalidInput);
    }
    if (params.offset_ms.is_some() && params.cursor.is_some())
        || params
            .display
            .as_ref()
            .is_some_and(|name| params.offset_ms.is_none() || name.is_empty() || name.len() > 255)
        || (params.speaker.is_some() && (params.offset_ms.is_some() || params.display.is_some()))
    {
        return Err(ToolError::InvalidInput);
    }
    let audio = match params.audio {
        Some(true) => {
            if params.cursor.is_some()
                || params.offset_ms.is_some()
                || params.display.is_some()
                || params.speaker.is_some()
            {
                return Err(ToolError::InvalidInput);
            }
            let start_ms = params.audio_start_ms.unwrap_or(0);
            if start_ms > MAX_AUDIO_START_MS {
                return Err(ToolError::InvalidInput);
            }
            let duration_ms = params
                .audio_duration_ms
                .unwrap_or(DEFAULT_AUDIO_DURATION_MS);
            if !(MIN_CLIP_DURATION_MS..=MAX_CLIP_DURATION_MS).contains(&duration_ms) {
                return Err(ToolError::InvalidInput);
            }
            if params
                .audio_track
                .as_ref()
                .is_some_and(|name| name.is_empty() || name.len() > 255)
            {
                return Err(ToolError::InvalidInput);
            }
            let encoding = match params.audio_encoding.as_deref() {
                None | Some("content") => AudioEncoding::Content,
                Some("base64") => AudioEncoding::Base64,
                Some(_) => return Err(ToolError::InvalidInput),
            };
            Some(AudioClipRequest {
                start_ms,
                duration_ms,
                track: params.audio_track,
                encoding,
            })
        }
        Some(false) | None => {
            if audio_options_present {
                return Err(ToolError::InvalidInput);
            }
            None
        }
    };
    Ok(ValidatedGetTranscript::Reference(
        TranscriptReferenceRequest {
            offset_ms: params.offset_ms,
            display: params.display,
            audio,
            reference,
            speaker: params.speaker,
            context_lines,
            cursor: params.cursor,
        },
    ))
}

fn validate_limit(limit: usize) -> Result<(), ToolError> {
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(ToolError::InvalidInput);
    }
    Ok(())
}

const NO_SUMMARY_NOTE: &str = "no readable entries matched this summary.";

#[derive(Deserialize)]
struct JsonlTextLine {
    #[serde(default)]
    text: Option<String>,
}

fn count_words(text: &str) -> u64 {
    text.split_whitespace()
        .filter(|token| token.chars().any(char::is_alphanumeric))
        .count() as u64
}

fn count_file_words(path: &Path) -> Result<u64, std::io::Error> {
    use std::io::BufRead;

    let file = fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let is_jsonl = path.extension().and_then(|ext| ext.to_str()) == Some("jsonl");
    let mut total = 0_u64;
    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        if is_jsonl {
            if let Ok(parsed) = serde_json::from_str::<JsonlTextLine>(&line)
                && let Some(content) = parsed.text
            {
                total = total.saturating_add(count_words(&content));
            }
        } else {
            total = total.saturating_add(count_words(&line));
        }
    }
    Ok(total)
}

fn month_key(day: &str) -> String {
    if day.len() >= 6 {
        format!("{}-{}", &day[..4], &day[4..6])
    } else {
        day.to_owned()
    }
}

#[derive(Default)]
struct CountBucket {
    count: u64,
    days: BTreeSet<String>,
    duration_sec: u64,
    words: u64,
    by_month: BTreeMap<String, u64>,
}

impl CountBucket {
    fn add_entry(&mut self, day: &str, duration_sec: u64, words: u64) {
        self.count = self.count.saturating_add(1);
        self.days.insert(day.to_owned());
        self.duration_sec = self.duration_sec.saturating_add(duration_sec);
        self.words = self.words.saturating_add(words);
        *self.by_month.entry(month_key(day)).or_insert(0) += 1;
    }

    fn insert_common(&self, obj: &mut Map<String, Value>) {
        obj.insert("count".to_owned(), json!(self.count));
        obj.insert("days".to_owned(), json!(self.days.len()));
        if self.duration_sec > 0 {
            obj.insert("duration_sec".to_owned(), json!(self.duration_sec));
        }
        if self.words > 0 {
            obj.insert("words".to_owned(), json!(self.words));
        }
        if let Some(first) = self.days.first() {
            obj.insert("first_day".to_owned(), json!(first));
        }
        if let Some(last) = self.days.last() {
            obj.insert("last_day".to_owned(), json!(last));
        }
    }
}

/// Cross-source held counts across a day or span, grouped by permitted content
/// kind and by provenance source label. Strictly enforces every category grant,
/// facet scope, body hide rule, and live source-release check so ungranted
/// categories and hidden/deleted items are never counted or disclosed.
pub(crate) fn execute_summary(
    journal_root: &Path,
    snapshot: &ConnectionReadSnapshot,
    request: &ValidatedListTranscripts,
) -> Result<Prepared, DispatchError> {
    let from = request.day.as_deref().or(request.day_from.as_deref());
    let to = request.day.as_deref().or(request.day_to.as_deref());

    let segments_boundary = segment_boundary(snapshot, request.facet_id.as_deref());
    let body_boundary = boundary(
        snapshot,
        Some(AdmittedCategory::Body),
        request.facet_id.as_deref(),
    );
    let holds_body = crate::body_source::holds_body(snapshot);
    let holds_womens_health = crate::body_source::holds_womens_health(snapshot);
    let hides = crate::body_source::BodyHides::read(journal_root);
    let declarations = FacetDeclarationSet::from_journal(journal_root).ok();
    let sources = source_labels(journal_root);

    let day_map =
        day_dirs(journal_root).map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
    let mut days: Vec<String> = if let Some(day) = request.day.as_deref() {
        if day_map.contains_key(day) {
            vec![day.to_owned()]
        } else {
            Vec::new()
        }
    } else {
        day_map
            .into_keys()
            .filter(|day| {
                from.is_none_or(|start| day.as_str() >= start)
                    && to.is_none_or(|end| day.as_str() <= end)
            })
            .collect()
    };
    days.sort_unstable();

    let mut by_kind: BTreeMap<AdmittedCategory, CountBucket> = BTreeMap::new();
    let mut by_source: BTreeMap<(String, AdmittedCategory), CountBucket> = BTreeMap::new();
    let mut all_days: BTreeSet<String> = BTreeSet::new();
    let mut targets: Vec<String> = Vec::new();
    let mut unreadable = 0_usize;
    let mut seen_workouts: BTreeSet<(String, String)> = BTreeSet::new();

    for day in &days {
        let day_segments =
            iter_segments_skipping_unreadable_streams(journal_root, PathOrDay::Day(day))
                .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        for segment in day_segments {
            let Ok(identity) = segment.record_identity() else {
                continue;
            };
            let stream = identity.stream;
            let name = identity.name;

            // Calendar notes are counted from the latest calendar listing below.
            if solstone_core_segment::is_calendar_stream(journal_root, stream) {
                continue;
            }

            if solstone_core_segment::is_body_only_segment(&segment) {
                if !holds_body {
                    continue;
                }
                let Some(ref decls) = declarations else {
                    continue;
                };
                let probe = segment_assignment_probe_path(day, stream, name);
                let classification = classify_source(journal_root, &probe, Some(stream), decls);
                if !body_boundary.allows_classification(journal_root, &classification) {
                    continue;
                }
                if hides
                    .shown(journal_root, day, stream, &segment, holds_womens_health)
                    .is_none()
                {
                    continue;
                }
                if !crate::source_release::check_segment_transcript_release(
                    journal_root,
                    day,
                    stream,
                    name,
                ) {
                    continue;
                }
                let counted = if crate::body_source::is_left_out_workout(&segment) {
                    let workout_id = crate::body_source::workout_id(&segment);
                    seen_workouts.insert((stream.to_owned(), workout_id))
                } else {
                    segment.path().join(BODY_RECORDS_FILE).is_file()
                        && !crate::body_source::holds_only_moved(segment.path())
                        && crate::body_source::grid_clock(name).is_some()
                };
                if counted {
                    let src_label = source_label(&sources, stream);
                    by_kind
                        .entry(AdmittedCategory::Body)
                        .or_default()
                        .add_entry(day, 0, 0);
                    by_source
                        .entry((src_label, AdmittedCategory::Body))
                        .or_default()
                        .add_entry(day, 0, 0);
                    all_days.insert(day.clone());
                    targets.push(format!("{day}/{stream}/{name}"));
                }
                continue;
            }

            let (Some(boundary), Some(decls)) = (&segments_boundary, &declarations) else {
                continue;
            };
            if crate::may_see::segment_shown_within(
                journal_root,
                boundary,
                decls,
                &hides,
                day,
                stream,
                name,
                &segment,
            )
            .is_none()
            {
                continue;
            }
            if !crate::source_release::check_segment_transcript_release(
                journal_root,
                day,
                stream,
                name,
            ) {
                continue;
            }
            let (permitted_files, has_screen) =
                match permitted_segment_files_for_summary(&segment, &snapshot.categories) {
                    Ok(found) => found,
                    Err(_) => {
                        unreadable += 1;
                        continue;
                    }
                };
            if permitted_files.is_empty() && !has_screen {
                continue;
            }

            let mut kinds_words: BTreeMap<AdmittedCategory, u64> = BTreeMap::new();
            let mut failed_file = false;
            for file in &permitted_files {
                let words = if matches!(
                    file.kind,
                    AdmittedCategory::Transcripts
                        | AdmittedCategory::Messages
                        | AdmittedCategory::Notes
                        | AdmittedCategory::Documents
                ) {
                    match count_file_words(&file.path) {
                        Ok(w) => w,
                        Err(_) => {
                            failed_file = true;
                            continue;
                        }
                    }
                } else {
                    0
                };
                let entry = kinds_words.entry(file.kind).or_insert(0);
                *entry = entry.saturating_add(words);
            }
            if failed_file {
                unreadable += 1;
            }
            if has_screen {
                kinds_words.entry(AdmittedCategory::ScreenText).or_insert(0);
            }
            if kinds_words.is_empty() {
                continue;
            }
            // Re-check release after file reads to avoid counting a segment
            // removed mid-read.
            if !crate::source_release::check_segment_transcript_release(
                journal_root,
                day,
                stream,
                name,
            ) {
                continue;
            }

            let seg_duration_sec = segment_clock(name).map_or(0, |clock| {
                u64::from(
                    clock
                        .exclusive_end_seconds
                        .saturating_sub(clock.start_seconds),
                )
            });
            let src_label = source_label(&sources, stream);
            for (kind, words) in kinds_words {
                let duration_sec = if matches!(
                    kind,
                    AdmittedCategory::Transcripts | AdmittedCategory::ScreenText
                ) {
                    seg_duration_sec
                } else {
                    0
                };
                by_kind
                    .entry(kind)
                    .or_default()
                    .add_entry(day, duration_sec, words);
                by_source
                    .entry((src_label.clone(), kind))
                    .or_default()
                    .add_entry(day, duration_sec, words);
            }
            all_days.insert(day.clone());
            targets.push(format!("{day}/{stream}/{name}"));
        }
    }

    // Calendar events belong to no facet; count active (latest non-removed)
    // scheduled events only for whole-journal connections without a facet
    // filter when the calendar category is granted.
    if matches!(snapshot.scope, ConnectionScope::WholeJournal)
        && request.facet_id.is_none()
        && snapshot.categories.contains(&AdmittedCategory::Calendar)
    {
        let hits = solstone_core_indexer_store::calendar_schedule::scheduled_event_hits(
            journal_root,
            from,
            to,
        )
        .map_err(|_| DispatchError::Tool(ToolError::FileUnreadable))?;
        let mut released_paths: BTreeMap<String, bool> = BTreeMap::new();
        // Track each unique (event, occurrence) with its earliest covered day
        // and source label so multi-day events count once while all covered
        // days count toward active `days`.
        let mut unique_events: BTreeMap<(String, String), (String, String)> = BTreeMap::new();
        let mut cal_days: BTreeSet<String> = BTreeSet::new();
        let mut source_days: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        let mut source_events: BTreeMap<String, BTreeSet<(String, String)>> = BTreeMap::new();
        let mut cal_targets: BTreeSet<String> = BTreeSet::new();

        for hit in hits {
            let released = *released_paths.entry(hit.path.clone()).or_insert_with(|| {
                let rel = hit.path.strip_prefix("chronicle/").unwrap_or(&hit.path);
                crate::source_release::check_indexed_source_release(journal_root, rel)
            });
            if !released {
                continue;
            }
            let stream_name = if !hit.stream.is_empty() {
                hit.stream.as_str()
            } else {
                hit.path.split('/').nth(1).unwrap_or("")
            };
            let src_label = source_label(&sources, stream_name);
            let event_key = (hit.event, hit.occurrence);
            unique_events
                .entry(event_key.clone())
                .and_modify(|(earliest_day, _)| {
                    if hit.day < *earliest_day {
                        *earliest_day = hit.day.clone();
                    }
                })
                .or_insert_with(|| (hit.day.clone(), src_label.clone()));
            cal_days.insert(hit.day.clone());
            source_days
                .entry(src_label.clone())
                .or_default()
                .insert(hit.day.clone());
            source_events
                .entry(src_label)
                .or_default()
                .insert(event_key);
            all_days.insert(hit.day);
            cal_targets.insert(hit.path);
        }

        if !unique_events.is_empty() {
            let cal_bucket = by_kind.entry(AdmittedCategory::Calendar).or_default();
            cal_bucket.count = unique_events.len() as u64;
            cal_bucket.days = cal_days;
            for (earliest_day, src_label) in unique_events.values() {
                *cal_bucket
                    .by_month
                    .entry(month_key(earliest_day))
                    .or_insert(0) += 1;
                let src_bucket = by_source
                    .entry((src_label.clone(), AdmittedCategory::Calendar))
                    .or_default();
                *src_bucket
                    .by_month
                    .entry(month_key(earliest_day))
                    .or_insert(0) += 1;
            }
            for (src_label, events) in source_events {
                let src_bucket = by_source
                    .entry((src_label.clone(), AdmittedCategory::Calendar))
                    .or_default();
                src_bucket.count = events.len() as u64;
                if let Some(days) = source_days.remove(&src_label) {
                    src_bucket.days = days;
                }
            }
            targets.extend(cal_targets);
        }
    }

    let mut kind_rows: Vec<(AdmittedCategory, CountBucket)> = by_kind.into_iter().collect();
    kind_rows.sort_by(|(a_kind, a), (b_kind, b)| {
        Reverse(a.count)
            .cmp(&Reverse(b.count))
            .then_with(|| a_kind.as_str().cmp(b_kind.as_str()))
    });
    let total_items: usize = kind_rows
        .iter()
        .map(|(_, bucket)| usize::try_from(bucket.count).unwrap_or(usize::MAX))
        .fold(0_usize, usize::saturating_add);
    let by_kind_json: Vec<Value> = kind_rows
        .into_iter()
        .map(|(kind, bucket)| {
            let mut obj = Map::new();
            obj.insert("kind".to_owned(), json!(kind.as_str()));
            bucket.insert_common(&mut obj);
            if !bucket.by_month.is_empty() {
                obj.insert("by_month".to_owned(), json!(bucket.by_month));
            }
            Value::Object(obj)
        })
        .collect();

    let mut source_rows: Vec<((String, AdmittedCategory), CountBucket)> =
        by_source.into_iter().collect();
    source_rows.sort_by(|((a_src, a_kind), a), ((b_src, b_kind), b)| {
        Reverse(a.count)
            .cmp(&Reverse(b.count))
            .then_with(|| a_src.cmp(b_src))
            .then_with(|| a_kind.as_str().cmp(b_kind.as_str()))
    });
    let by_source_json: Vec<Value> = source_rows
        .into_iter()
        .map(|((source, kind), bucket)| {
            let mut obj = Map::new();
            obj.insert("source".to_owned(), json!(source));
            obj.insert("kind".to_owned(), json!(kind.as_str()));
            bucket.insert_common(&mut obj);
            Value::Object(obj)
        })
        .collect();

    let mut response = json!({
        "total_days": all_days.len(),
        "by_kind": by_kind_json,
        "by_source": by_source_json,
    });
    let range_start = from
        .map(str::to_owned)
        .or_else(|| all_days.first().cloned());
    let range_end = to.map(str::to_owned).or_else(|| all_days.last().cloned());
    if let (Some(start), Some(end)) = (range_start, range_end) {
        response["range"] = json!({ "start": start, "end": end });
    }
    if unreadable > 0 {
        response["unreadable"] = json!(unreadable);
    }

    Ok(Prepared::new(response, total_items, targets).when_empty(NO_SUMMARY_NOTE))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dispatch::run_mcp_probe;
    use crate::permissions::{PermissionStore, ReadPermission, ReadScope};
    use sha2::{Digest, Sha256};
    use solstone_core_format::content::body_hidden_days::{hidden_days_bytes, hidden_days_path};

    const CONN: &str = "bearer:transcript-summary-test";
    const FACET_A: &str = "123e4567-e89b-42d3-a456-426614174000";
    const FACET_B: &str = "123e4567-e89b-42d3-a456-426614174001";

    fn write_file(root: &Path, rel: &str, body: &str) {
        let path = root.join(rel);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).unwrap();
        }
        fs::write(path, body).unwrap();
    }

    fn grant(root: &Path, categories: &[&str], scope: ReadScope) {
        PermissionStore::open(root)
            .set_permission(
                CONN,
                ReadPermission {
                    categories: categories.iter().map(|s| (*s).to_owned()).collect(),
                    scope,
                },
            )
            .unwrap();
    }

    #[test]
    fn validate_list_summary_rejects_incompatible_arguments() {
        assert!(
            validate_list(Some(&json!({
                "day_from": "20250101",
                "day_to": "20251231",
                "summary": true
            })))
            .is_ok()
        );
        for bad in [
            json!({"day": "20250101", "summary": true, "limit": 5}),
            json!({"day": "20250101", "summary": true, "cursor": "tok"}),
            json!({"day": "20250101", "summary": true, "time_from": "08:00"}),
            json!({"day": "20250101", "summary": true, "source": "studio talk"}),
            json!({"day": "20250101", "summary": true, "entry_kind": "sleep"}),
        ] {
            assert_eq!(
                validate_list(Some(&bad)).err(),
                Some(ToolError::InvalidInput),
                "{bad}"
            );
        }
    }

    #[test]
    fn summary_counts_respect_grants_hides_deletions_facets_and_owner_labels() {
        let dir = tempfile::Builder::new()
            .prefix("solstone-mcp-summary-")
            .tempdir_in(crate::test_scratch())
            .unwrap();
        let root = dir.path();

        write_file(
            root,
            "config/journal.json",
            &json!({"identity": {"timezone": "UTC"}}).to_string(),
        );
        for (folder, id, title) in [("alpha", FACET_A, "Alpha"), ("beta", FACET_B, "Beta")] {
            write_file(
                root,
                &format!("facets/{folder}/facet.json"),
                &json!({"id": id, "title": title, "description": title}).to_string(),
            );
        }

        // Streams: two owner-labeled device:None audio streams (including one
        // whose owner_label contains its stream stem), screen, obsidian,
        // document, image, apple_health, and calendar.
        for (name, kind, cid, source) in [
            ("talk", "observer", Some("sha256:t"), Some("microphone")),
            ("memos", "observer", Some("sha256:m"), Some("voice_memo")),
            ("desk", "observer", Some("sha256:d"), Some("screen")),
            ("obsidian", "observer", Some("sha256:o"), Some("obsidian")),
            ("calendar", "observer", Some("sha256:c"), Some("calendar")),
            ("import.document", "import", None, None),
            ("import.image", "import", None, None),
            ("import.apple_health", "import", None, None),
        ] {
            let mut rec = json!({
                "name": name, "kind": kind, "host": null, "platform": null,
                "created_at": 1, "last_day": null, "last_segment": null, "seq": 1
            });
            if let Some(cid) = cid {
                rec["cid"] = json!(cid);
            }
            if let Some(source) = source {
                rec["source"] = json!(source);
            }
            write_file(root, &format!("streams/{name}.json"), &rec.to_string());
        }
        write_file(
            root,
            "link/client-descriptions.json",
            &json!({
                "sha256:t": {"protocol_version":1,"revision":1,"reported":null,"owner_label":"studio talk","updated_at":null},
                "sha256:m": {"protocol_version":1,"revision":1,"reported":null,"owner_label":"field memos","updated_at":null},
                "sha256:d": {"protocol_version":1,"revision":1,"reported":null,"owner_label":"work desk","updated_at":null},
                "sha256:o": {"protocol_version":1,"revision":1,"reported":null,"owner_label":"notebook","updated_at":null}
            })
            .to_string(),
        );

        let assign_facet = |day: &str, stream: &str, seg: &str, facet: &str| {
            write_file(
                root,
                &format!("chronicle/{day}/{stream}/{seg}/stream.json"),
                &json!({"stream": stream}).to_string(),
            );
            write_file(
                root,
                &format!("chronicle/{day}/{stream}/{seg}/talents/facets.json"),
                &json!([{"facet": facet}]).to_string(),
            );
        };

        // 20250105 talk (alpha, 180s, 5 words) + memos (beta, 60s, 3 words)
        assign_facet("20250105", "talk", "090000_180", "alpha");
        write_file(
            root,
            "chronicle/20250105/talk/090000_180/audio.jsonl",
            "{\"backend\":\"whisper\"}\n{\"sentence_id\":1,\"speaker\":\"owner\",\"text\":\"hello world from morning talk\"}\n",
        );
        assign_facet("20250105", "memos", "100000_60", "beta");
        write_file(
            root,
            "chronicle/20250105/memos/100000_60/audio.jsonl",
            "{\"backend\":\"whisper\"}\n{\"sentence_id\":1,\"speaker\":\"owner\",\"text\":\"quick voice memo\"}\n",
        );

        // Deleted/tombstoned talk segment must never be counted.
        assign_facet("20250105", "talk", "110000_120", "alpha");
        write_file(
            root,
            "chronicle/20250105/talk/110000_120/audio.jsonl",
            "{\"sentence_id\":1,\"speaker\":\"owner\",\"text\":\"deleted words never counted\"}\n",
        );
        write_file(
            root,
            "chronicle/20250105/talk/110000_120/tombstone.json",
            "{}",
        );

        // 20250105 desk (alpha, 300s screen_text), obsidian note (alpha, 4 words),
        // 20250210 document (beta, 3 words), photo (alpha), body window (visible + hidden day).
        assign_facet("20250105", "desk", "130000_300", "alpha");
        write_file(
            root,
            "chronicle/20250105/desk/130000_300/talents/screen_text.md",
            "drafting scene one",
        );
        assign_facet("20250105", "obsidian", "140000_300", "alpha");
        write_file(
            root,
            "chronicle/20250105/obsidian/140000_300/note_transcript.md",
            "# Heading\n\nThree body words.\n",
        );
        assign_facet("20250210", "import.document", "120000_0", "beta");
        write_file(
            root,
            "chronicle/20250210/import.document/120000_0/document_transcript.md",
            "Signed lease agreement.",
        );
        assign_facet("20250210", "import.image", "150000_0", "alpha");
        write_file(
            root,
            "chronicle/20250210/import.image/150000_0/image_transcript.md",
            "# River bridge\n\n**Type:** Image\n",
        );
        // Body windows: 20250105 visible, 20250106 hidden.
        write_file(
            root,
            "chronicle/20250105/import.apple_health/060000_300/body.jsonl",
            &format!(
                "{}\n",
                json!({"v":1,"id":"s1","kind":"count","source":"apple_health","type":"HKQuantityTypeIdentifierStepCount","label":"step count","value":"5000","unit":"count","start":"2025-01-05T09:00:00Z","end":"2025-01-05T10:00:00Z"})
            ),
        );
        write_file(
            root,
            "chronicle/20250106/import.apple_health/060000_300/body.jsonl",
            &format!(
                "{}\n",
                json!({"v":1,"id":"s2","kind":"count","source":"apple_health","type":"HKQuantityTypeIdentifierStepCount","label":"step count","value":"9000","unit":"count","start":"2025-01-06T09:00:00Z","end":"2025-01-06T10:00:00Z"})
            ),
        );
        let hidden_path = hidden_days_path(root);
        fs::create_dir_all(hidden_path.parent().unwrap()).unwrap();
        fs::write(
            &hidden_path,
            hidden_days_bytes(&BTreeSet::from(["20250106".to_owned()])),
        )
        .unwrap();

        // Calendar: 1 active event + 1 later-removed event (removed must NOT be counted).
        let cal_active = json!({
            "v": 1, "event": "ev-keep", "occurrence": null, "change": "added",
            "changed_at": "2025-01-01T07:00:00+00:00", "calendar": "Work",
            "title": "Rehearsal", "all_day": false,
            "start": "2025-01-05T14:00:00+00:00", "end": "2025-01-05T15:00:00+00:00",
            "time_zone": "UTC", "location": null, "notes": null, "url": null,
            "organizer": null, "attendees": [], "repeats": null
        })
        .to_string();
        let cal_removed_add = json!({
            "v": 1, "event": "ev-drop", "occurrence": null, "change": "added",
            "changed_at": "2025-01-01T07:00:00+00:00", "calendar": "Work",
            "title": "Cancelled", "all_day": false,
            "start": "2025-02-12T10:00:00+00:00", "end": "2025-02-12T11:00:00+00:00",
            "time_zone": "UTC", "location": null, "notes": null, "url": null,
            "organizer": null, "attendees": [], "repeats": null
        })
        .to_string();
        let cal_removed_del = json!({
            "v": 1, "event": "ev-drop", "occurrence": null, "change": "removed",
            "changed_at": "2025-02-10T07:00:00+00:00", "calendar": "Work",
            "title": "Cancelled", "all_day": null,
            "start": null, "end": null, "time_zone": null, "location": null,
            "notes": null, "url": null, "organizer": null, "attendees": [], "repeats": null
        })
        .to_string();
        for (day, seg, content) in [
            ("20250101", "070000_300", format!("{cal_active}\n")),
            ("20250101", "071000_300", format!("{cal_removed_add}\n")),
            ("20250210", "070000_300", format!("{cal_removed_del}\n")),
        ] {
            let hash = format!("{:x}", Sha256::digest(content.as_bytes()));
            write_file(
                root,
                &format!("chronicle/{day}/calendar/{seg}/stream.json"),
                r#"{"stream":"calendar"}"#,
            );
            write_file(
                root,
                &format!("chronicle/{day}/calendar/{seg}/calendar_{hash}.jsonl"),
                &content,
            );
        }
        solstone_core_indexer_store::scan::scan_journal(root, true).unwrap();

        // 1. Full whole-journal grant: verifies every kind, owner labels, counts,
        // durations, words, hidden body day suppression, tombstoned segment
        // suppression, and removed calendar event suppression.
        grant(
            root,
            &[
                "transcripts",
                "screen_text",
                "screenshots",
                "notes",
                "documents",
                "photos",
                "body",
                "calendar",
            ],
            ReadScope::WholeJournal,
        );
        let full = run_mcp_probe(
            root,
            CONN,
            "list_transcripts",
            &json!({"day_from": "20250101", "day_to": "20250228", "summary": true}),
        )
        .unwrap();
        assert_eq!(full["total_days"], json!(2));
        let kind_entry = |k: &str| {
            full["by_kind"]
                .as_array()
                .unwrap()
                .iter()
                .find(|row| row["kind"] == k)
                .cloned()
        };
        let transcripts = kind_entry("transcripts").expect("transcripts kind");
        assert_eq!(transcripts["count"], json!(2));
        assert_eq!(transcripts["days"], json!(1));
        assert_eq!(transcripts["duration_sec"], json!(240));
        assert_eq!(transcripts["words"], json!(8));
        assert_eq!(transcripts["by_month"]["2025-01"], json!(2));

        let screen = kind_entry("screen_text").expect("screen_text kind");
        assert_eq!(screen["count"], json!(1));
        assert_eq!(screen["duration_sec"], json!(300));
        let notes = kind_entry("notes").expect("notes kind");
        assert_eq!(notes["count"], json!(1));
        assert_eq!(notes["words"], json!(4));
        assert!(notes.get("duration_sec").is_none());
        let docs = kind_entry("documents").expect("documents kind");
        assert_eq!(docs["count"], json!(1));
        assert_eq!(docs["words"], json!(3));
        let photos = kind_entry("photos").expect("photos kind");
        assert_eq!(photos["count"], json!(1));
        assert_eq!(photos["by_month"]["2025-02"], json!(1));
        let body = kind_entry("body").expect("body kind");
        assert_eq!(body["count"], json!(1), "hidden 20250106 body excluded");
        let calendar = kind_entry("calendar").expect("calendar kind");
        assert_eq!(
            calendar["count"],
            json!(1),
            "removed calendar event excluded"
        );
        assert_eq!(calendar["days"], json!(1));

        let by_source = full["by_source"].as_array().unwrap();
        assert!(
            by_source.iter().any(|s| s["source"] == "studio talk"
                && s["kind"] == "transcripts"
                && s["count"] == 1
                && s["words"] == 5),
            "owner_label missing: {by_source:?}"
        );
        assert!(
            by_source.iter().any(|s| s["source"] == "field memos"
                && s["kind"] == "transcripts"
                && s["count"] == 1
                && s["words"] == 3),
            "owner_label missing: {by_source:?}"
        );
        assert!(
            by_source.iter().any(|s| s["source"] == "work desk · screen"
                && s["kind"] == "screen_text"
                && s["count"] == 1),
            "screen owner_label missing: {by_source:?}"
        );
        assert!(
            !full.to_string().contains("your device ("),
            "no generic numbered collapse: {full}"
        );

        // 2. Round-trip: source label from list_transcripts/summary filters list_transcripts(source=...).
        let filtered = run_mcp_probe(
            root,
            CONN,
            "list_transcripts",
            &json!({"day": "20250105", "source": "studio talk"}),
        )
        .unwrap();
        let rows = filtered["transcripts"].as_array().unwrap();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0]["source"], json!("studio talk"));

        // 3. Narrow category grant (transcripts + notes only): never leaks photos, docs, screen, body, calendar.
        grant(root, &["transcripts", "notes"], ReadScope::WholeJournal);
        let narrow = run_mcp_probe(
            root,
            CONN,
            "list_transcripts",
            &json!({"day_from": "20250101", "day_to": "20250228", "summary": true}),
        )
        .unwrap();
        let narrow_kinds: BTreeSet<&str> = narrow["by_kind"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["kind"].as_str())
            .collect();
        assert_eq!(narrow_kinds, BTreeSet::from(["transcripts", "notes"]));
        assert_eq!(narrow["total_days"], json!(1));

        // 4. Facet-scoped connection (FACET_A only): excludes beta segments AND excludes calendar.
        grant(
            root,
            &[
                "transcripts",
                "screen_text",
                "screenshots",
                "notes",
                "documents",
                "photos",
                "body",
                "calendar",
            ],
            ReadScope::Facets {
                ids: vec![FACET_A.to_owned()],
            },
        );
        let scoped = run_mcp_probe(
            root,
            CONN,
            "list_transcripts",
            &json!({"day_from": "20250101", "day_to": "20250228", "summary": true}),
        )
        .unwrap();
        let scoped_kinds: BTreeSet<&str> = scoped["by_kind"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["kind"].as_str())
            .collect();
        assert!(
            !scoped_kinds.contains("calendar"),
            "calendar belongs to no facet"
        );
        assert!(
            !scoped_kinds.contains("documents"),
            "beta document must not leak"
        );
        let scoped_sources: BTreeSet<&str> = scoped["by_source"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["source"].as_str())
            .collect();
        assert!(scoped_sources.contains("studio talk"));
        assert!(
            !scoped_sources.contains("field memos"),
            "beta stream must not leak"
        );
    }
}
