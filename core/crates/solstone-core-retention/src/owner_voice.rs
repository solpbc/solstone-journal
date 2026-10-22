// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Deleting the original audio of a recording transcribed in voice-only mode.
//!
//! When transcription keeps only the owner's speech, its record names the original file
//! it processed (`owner_voice.original`). Once those words are saved, the original is
//! deleted through the door's ordinary raw release with the owner-voice class, under the
//! transcript's output claim and segment commit, after re-verifying the original's size and
//! digest against the record. A changed original is recorded as a permanent refusal and is
//! never deleted. A deletion that cannot finish yet stays owed, and the daily cleanup
//! routine finishes it for recordings dated within [`owner_voice_owed_days`].

use std::fs;
use std::io;
use std::path::{Component, Path};

use chrono::{DateTime, Days, NaiveDate, Utc};
use serde_json::{Map, Value};
use solstone_core_callosum::{CallosumEnvelope, DurableEvent, append_durable_event};
use solstone_core_journal_io::paths::{DEFAULT_STREAM, contained_path, path_lexists};
use solstone_core_processing_record::{OwnerVoiceFilter, owner_voice, read_jsonl_header, vocab};
use solstone_core_segment::output_attempt::{InputObservation, OutputClaims, SegmentCommit};

use crate::content::{ClosedHandlerSet, ContentName};
use crate::eligibility::{FileVerdict, ProvenRaw, resolve_file};
use crate::original_deletion::RawReleaseClass;
use crate::scan::read_sidecar;

const EVENTS_FILE: &str = "events.jsonl";
const RETENTION_TRACT: &str = "retention";
const ORIGINAL_DELETED_EVENT: &str = "original_deleted";
const RELEASE_REFUSED_EVENT: &str = "original_release_refused";
/// Days, counting today, that owed owner-voice deletions are retried.
pub const OWNER_VOICE_OWED_DAYS: u64 = 30;

/// Why an owner-voice original is kept for good.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwnerVoiceRefusal {
    /// The original's size differs from the one the transcript processed.
    SizeChanged,
    /// The original's bytes differ from the ones the transcript processed.
    BytesChanged,
}

impl OwnerVoiceRefusal {
    #[must_use]
    pub fn tag(self) -> &'static str {
        match self {
            Self::SizeChanged => "size_changed",
            Self::BytesChanged => "bytes_changed",
        }
    }
}

/// What a segment's retention events record about one file name.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct RetentionEventsForName {
    /// Some `original_deleted` row names this file, whatever its class.
    pub deleted: bool,
    /// Some `original_release_refused` row names this file, whatever its reason.
    pub refused: bool,
}

/// Scan a segment's `events.jsonl` for retention rows naming `name`.
///
/// An absent events file is an empty log. A symlink or other non-file at that name, or a
/// read error, is an error. A torn or unparseable line is skipped.
pub fn retention_events_for_name(
    segment_dir: &Path,
    name: &str,
) -> io::Result<RetentionEventsForName> {
    let path = segment_dir.join(EVENTS_FILE);
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok(RetentionEventsForName::default());
        }
        Err(error) => return Err(error),
    };
    if !metadata.file_type().is_file() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "segment events are not a regular file",
        ));
    }
    let bytes = fs::read(&path)?;
    let mut found = RetentionEventsForName::default();
    for line in bytes.split(|byte| *byte == b'\n') {
        let Ok(text) = std::str::from_utf8(line) else {
            continue;
        };
        let Ok(Value::Object(row)) = serde_json::from_str::<Value>(text.trim()) else {
            continue;
        };
        if row.get("tract").and_then(Value::as_str) != Some(RETENTION_TRACT)
            || row.get("name").and_then(Value::as_str) != Some(name)
        {
            continue;
        }
        match row.get("event").and_then(Value::as_str) {
            Some(ORIGINAL_DELETED_EVENT) => found.deleted = true,
            Some(RELEASE_REFUSED_EVENT) => found.refused = true,
            _ => {}
        }
    }
    Ok(found)
}

/// Record that an owner-voice original is kept for good.
pub fn record_owner_voice_release_refused(
    journal: &Path,
    segment_rel: &str,
    name: &str,
    reason: OwnerVoiceRefusal,
    at: DateTime<Utc>,
) -> Result<(), String> {
    let resolved_dir = contained_path(journal, segment_rel).map_err(|error| error.to_string())?;
    let mut extra = Map::new();
    extra.insert("name".to_owned(), Value::String(name.to_owned()));
    extra.insert(
        "class".to_owned(),
        Value::String(RawReleaseClass::OwnerVoice.tag().to_owned()),
    );
    extra.insert("reason".to_owned(), Value::String(reason.tag().to_owned()));
    let envelope = CallosumEnvelope {
        tract: RETENTION_TRACT.to_owned(),
        event: RELEASE_REFUSED_EVENT.to_owned(),
        ts: Some(at.timestamp_millis()),
        extra,
    };
    append_durable_event(&resolved_dir, &DurableEvent::Callosum(envelope))
        .map_err(|error| error.to_string())
}

/// Whether a transcript's original still has to be deleted.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OwedState {
    Owed,
    NotOwed,
    /// The segment's events could not be read; treated as not owed, so the original is kept.
    Unknown,
}

/// Whether the original named by a voice-only transcript still has to be deleted.
///
/// Reads the transcript's record and the segment's events, never the original's size or
/// bytes (re-verifying those is the caller's job). Owed only when the record is a terminal
/// transcription (an analyzed one still carrying its analysis rows), its `owner_voice`
/// member is valid and names the header's `raw` file, that file exists, and no
/// `original_deleted` or `original_release_refused` event names it.
#[must_use]
pub fn owner_voice_deletion_state(segment_dir: &Path, jsonl: &Path) -> OwedState {
    let Some(filter) = owed_filter(jsonl) else {
        return OwedState::NotOwed;
    };
    let name = filter.original.name.as_str();
    match path_lexists(&segment_dir.join(name)) {
        Ok(true) => {}
        Ok(false) => return OwedState::NotOwed,
        Err(_) => return OwedState::Unknown,
    }
    match retention_events_for_name(segment_dir, name) {
        Ok(events) if events.deleted || events.refused => OwedState::NotOwed,
        Ok(_) => OwedState::Owed,
        Err(_) => OwedState::Unknown,
    }
}

/// Whether the original named by a voice-only transcript still has to be deleted.
#[must_use]
pub fn owner_voice_deletion_owed(segment_dir: &Path, jsonl: &Path) -> bool {
    owner_voice_deletion_state(segment_dir, jsonl) == OwedState::Owed
}

fn owed_filter(jsonl: &Path) -> Option<OwnerVoiceFilter> {
    let header = read_jsonl_header(jsonl)?;
    let record = header.get("_solstone_processing")?;
    if record.get("schema").and_then(Value::as_str) != Some(vocab::SCHEMA)
        || record.get("handler").and_then(Value::as_str) != Some(vocab::HANDLER_TRANSCRIBE)
    {
        return None;
    }
    let state = record.get("state").and_then(Value::as_str)?;
    if state != vocab::STATE_ANALYZED && state != vocab::STATE_EMPTY {
        return None;
    }
    let filter = owner_voice(record)?.ok()?;
    if header.get("raw").and_then(Value::as_str) != Some(filter.original.name.as_str()) {
        return None;
    }
    if state == vocab::STATE_ANALYZED
        && !read_sidecar(jsonl, Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY)).has_analysis_row
    {
        return None;
    }
    Some(filter)
}

/// The days, in the owner's zone, whose recordings have owed deletions retried: `today`
/// and the days before it, [`OWNER_VOICE_OWED_DAYS`] in all.
pub fn owner_voice_owed_days(today: NaiveDate) -> impl Iterator<Item = NaiveDate> {
    (0..OWNER_VOICE_OWED_DAYS).filter_map(move |back| today.checked_sub_days(Days::new(back)))
}

/// What finishing an owner-voice deletion did.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OwnerVoiceDeletion {
    /// The original was deleted and its deletion recorded.
    Deleted,
    /// The original was deleted, but its event could not be written.
    DeletedUnrecorded,
    /// The original was already gone; nothing was written.
    Gone,
    /// Nothing is owed for this transcript.
    NotOwed,
    /// The segment's events could not be read; the original is kept.
    Unknown,
    /// The original changed since it was transcribed; it is kept for good.
    Refused(OwnerVoiceRefusal),
    /// The deletion could not be proven for this exact file; nothing was deleted or recorded.
    NotProvable(String),
    /// The deletion could not finish now and is still owed.
    Owed(String),
}

/// Finish the deletion of a voice-only transcript's original.
///
/// The caller holds `claims` on exactly `jsonl` and `commit` from those claims on the
/// transcript's segment, and holds no open handle to the original. Both stay held through
/// the call, so no cooperating writer can replace the original between the check and the
/// removal.
pub fn finish_owner_voice_deletion(
    journal: &Path,
    jsonl: &Path,
    claims: &OutputClaims,
    commit: &SegmentCommit<'_>,
    at: DateTime<Utc>,
) -> OwnerVoiceDeletion {
    let Some(segment) = jsonl.parent() else {
        return OwnerVoiceDeletion::NotProvable("transcript has no segment".to_owned());
    };
    if let Err(error) = commit.require_outputs(claims, segment, [jsonl.to_path_buf()]) {
        return OwnerVoiceDeletion::NotProvable(error.to_string());
    }
    match owner_voice_deletion_state(segment, jsonl) {
        OwedState::Owed => {}
        OwedState::NotOwed => return OwnerVoiceDeletion::NotOwed,
        OwedState::Unknown => return OwnerVoiceDeletion::Unknown,
    }
    let Some(filter) = owed_filter(jsonl) else {
        return OwnerVoiceDeletion::NotOwed;
    };
    let name = filter.original.name.clone();
    let original = segment.join(&name);
    let Some((day, stream, dir)) = segment_coordinates(journal, segment) else {
        return OwnerVoiceDeletion::NotProvable("segment is not inside the journal".to_owned());
    };
    let segment_rel = crate::layout::segment_rel(&day, &stream, &dir);

    // The observation lives only inside this block: the handle on the original closes when
    // the block ends, before the release (an open handle would leave the name delete-pending
    // on Windows).
    let verified = {
        let observation = match InputObservation::capture(claims, &original) {
            Ok(observation) => observation,
            Err(_) if matches!(path_lexists(&original), Ok(false)) => {
                return OwnerVoiceDeletion::Gone;
            }
            Err(error) => return OwnerVoiceDeletion::Owed(error.to_string()),
        };
        if observation.input_size() != filter.original.size {
            Err(OwnerVoiceRefusal::SizeChanged)
        } else if hex(&observation.input_sha256()) != filter.original.sha256 {
            Err(OwnerVoiceRefusal::BytesChanged)
        } else {
            Ok(observation.input_size())
        }
    };
    let size = match verified {
        Ok(size) => size,
        Err(reason) => {
            return match record_owner_voice_release_refused(
                journal,
                &segment_rel,
                &name,
                reason,
                at,
            ) {
                Ok(()) => OwnerVoiceDeletion::Refused(reason),
                Err(error) => OwnerVoiceDeletion::Owed(error),
            };
        }
    };

    let Some(content) = ContentName::new(&name) else {
        return OwnerVoiceDeletion::NotProvable("original has no media name".to_owned());
    };
    let facts = read_sidecar(jsonl, Some(vocab::AUDIO_TRANSCRIPT_ROW_KEY));
    let proven = match resolve_file(
        &ClosedHandlerSet,
        &day,
        &stream,
        &dir,
        &content,
        size,
        &facts,
    ) {
        FileVerdict::Releasable(proven) => proven,
        FileVerdict::Held(_) => {
            return OwnerVoiceDeletion::NotProvable(
                "the transcript does not prove this original".to_owned(),
            );
        }
    };
    if journal.join(proven.rel()) != original {
        return OwnerVoiceDeletion::NotProvable("proof names a different path".to_owned());
    }
    release_proven(journal, segment, &proven, &name, at)
}

fn release_proven(
    journal: &Path,
    segment: &Path,
    proven: &ProvenRaw,
    name: &str,
    at: DateTime<Utc>,
) -> OwnerVoiceDeletion {
    let (outcome, _tally) = crate::door::release_raw(
        journal,
        std::slice::from_ref(proven),
        RawReleaseClass::OwnerVoice,
        at,
    );
    // Decide by what is now true on disk, not by the receipt's prose.
    // An unknown answer counts as still present: the deletion stays owed.
    if !matches!(path_lexists(&segment.join(name)), Ok(false)) {
        let detail = outcome
            .targets
            .iter()
            .flat_map(|target| target.not_removed.iter())
            .map(|row| row.reason.clone())
            .next()
            .unwrap_or_else(|| "the original is still present".to_owned());
        return OwnerVoiceDeletion::Owed(detail);
    }
    match retention_events_for_name(segment, name) {
        Ok(events) if events.deleted => OwnerVoiceDeletion::Deleted,
        _ => OwnerVoiceDeletion::DeletedUnrecorded,
    }
}

/// Counts from one cleanup run, for its report.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct OwedRunCounts {
    pub deleted: u64,
    pub deleted_unrecorded: u64,
    pub gone: u64,
    pub owed: u64,
    pub refused: u64,
    pub not_provable: u64,
    pub not_owed: u64,
    pub skipped: u64,
    pub unknown: u64,
}

/// Finish owed owner-voice deletions for recordings dated in [`owner_voice_owed_days`].
///
/// Reads only transcript headers, and only where the named original still exists. Each
/// deletion acquires the transcript's claim and the segment commit with zero wait and is
/// skipped on contention (the next run retries). Deletions still owed after the pass are
/// retried up to `retries` times, `pause` apart, holding no claim while waiting. Runs whatever
/// the voice-only setting is now: deletion follows each record's saved intent.
pub fn finish_owed_owner_voice_deletions(
    journal: &Path,
    today: NaiveDate,
    retries: usize,
    pause: std::time::Duration,
    now: &dyn Fn() -> DateTime<Utc>,
) -> OwedRunCounts {
    let mut counts = OwedRunCounts::default();
    let mut owed = Vec::new();
    for day in owner_voice_owed_days(today) {
        let key = day.format("%Y%m%d").to_string();
        let Ok(segments) = solstone_core_segment::list_segments(journal, &key) else {
            continue;
        };
        for segment in segments {
            let Ok(entries) = fs::read_dir(segment.path()) else {
                continue;
            };
            for entry in entries.flatten() {
                let jsonl = entry.path();
                if jsonl.extension().and_then(|value| value.to_str()) != Some("jsonl") {
                    continue;
                }
                match owner_voice_deletion_state(segment.path(), &jsonl) {
                    OwedState::Owed => owed.push(jsonl),
                    OwedState::Unknown => counts.unknown = counts.unknown.saturating_add(1),
                    OwedState::NotOwed => {}
                }
            }
        }
    }
    for attempt in 0..=retries {
        if owed.is_empty() {
            break;
        }
        if attempt > 0 {
            std::thread::sleep(pause);
        }
        let last = attempt == retries;
        let mut still_owed = Vec::new();
        for jsonl in owed {
            match finish_one(journal, &jsonl, now()) {
                Some(OwnerVoiceDeletion::Owed(_)) if !last => still_owed.push(jsonl),
                Some(outcome) => tally(&mut counts, &outcome),
                None => counts.skipped = counts.skipped.saturating_add(1),
            }
        }
        owed = still_owed;
    }
    counts
}

fn finish_one(journal: &Path, jsonl: &Path, at: DateTime<Utc>) -> Option<OwnerVoiceDeletion> {
    let segment = jsonl.parent()?;
    let options = solstone_core_journal_io::LockOptions {
        timeout: std::time::Duration::ZERO,
        ..solstone_core_journal_io::LockOptions::default()
    };
    let claims = OutputClaims::acquire([jsonl.to_path_buf()], options).ok()?;
    let commit = claims.commit(segment).ok()?;
    Some(finish_owner_voice_deletion(
        journal, jsonl, &claims, &commit, at,
    ))
}

fn tally(counts: &mut OwedRunCounts, outcome: &OwnerVoiceDeletion) {
    let slot = match outcome {
        OwnerVoiceDeletion::Deleted => &mut counts.deleted,
        OwnerVoiceDeletion::DeletedUnrecorded => &mut counts.deleted_unrecorded,
        OwnerVoiceDeletion::Gone => &mut counts.gone,
        OwnerVoiceDeletion::NotOwed => &mut counts.not_owed,
        OwnerVoiceDeletion::Unknown => &mut counts.unknown,
        OwnerVoiceDeletion::Refused(_) => &mut counts.refused,
        OwnerVoiceDeletion::NotProvable(_) => &mut counts.not_provable,
        OwnerVoiceDeletion::Owed(_) => &mut counts.owed,
    };
    *slot = slot.saturating_add(1);
}

/// `(day, stream, dir)` of a segment directory inside the journal's chronicle, for either
/// layout: `chronicle/<day>/<dir>` (the default stream) or `chronicle/<day>/<stream>/<dir>`.
fn segment_coordinates(journal: &Path, segment: &Path) -> Option<(String, String, String)> {
    let relative = segment.strip_prefix(journal).ok()?;
    let parts = relative
        .components()
        .map(|component| match component {
            Component::Normal(part) => part.to_str().map(str::to_owned),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    match parts.as_slice() {
        [chronicle, day, dir] if chronicle == "chronicle" => {
            Some((day.clone(), DEFAULT_STREAM.to_owned(), dir.clone()))
        }
        [chronicle, day, stream, dir] if chronicle == "chronicle" => {
            Some((day.clone(), stream.clone(), dir.clone()))
        }
        _ => None,
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
#[path = "owner_voice/tests.rs"]
mod tests;
