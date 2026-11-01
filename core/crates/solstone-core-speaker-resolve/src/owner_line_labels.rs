// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Set, remove and restore the owner's own answers to "who said this line".
//!
//! The answers live in each segment's `talents/owner_line_labels.json`
//! (format and read: `solstone_core_speaker_id::owner_line_labels`). Nothing
//! here reads or writes the segment's speaker labels, its corrections, any
//! voiceprint or any owner-voice state: an answer is the owner's word about
//! who spoke, never voice evidence.
//!
//! Every write holds the journal's entity trust lock across validation and
//! write, then the file's own lock, the order the owner's speaker fixes take
//! them. The segment directory is captured and revalidated before anything is
//! created under it, and a deleted, tombstoned or staged segment is refused;
//! nothing here recreates a segment, its `talents/` directory or a lock after
//! a delete. A multi-segment change checks every segment and line before it
//! writes any, and puts back the segments it already wrote if a later one
//! fails, so a refused or failed change leaves every owner file as it was.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_entity::{
    IdentityObservation, entity_merge_recovery_pending, hold_entity_trust_lock,
    is_admissible_person, live_merge_successor, load_all_journal_entities, merged_away,
    observe_entity_identity,
};
use solstone_core_journal_io::{FileLock, LockOptions};
use solstone_core_segment::{OwnerDeletionState, owner_deletion_state};
use solstone_core_speaker_id::owner_line_labels::{
    LineFingerprint, OWNER_LINE_LABELS_FILE, OwnerLineEntry, decode_owner_line_labels,
    encode_owner_line_labels,
};
use thiserror::Error;

use crate::owner_admission::{OwnerAdmission, admitted_owner_id};

/// Who the owner says spoke a line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LineSpeaker {
    /// The journal's owner, resolved to the admitted owner identity.
    Owner,
    /// A person in the journal, by entity id.
    Person(String),
    /// Someone the owner doesn't know.
    Unknown,
}

impl LineSpeaker {
    /// Read the tagged form a client sends: `{"kind":"owner"}`,
    /// `{"kind":"person","id":"<entity id>"}` or `{"kind":"unknown"}`.
    pub fn from_tagged(value: &Value) -> Option<Self> {
        let object = value.as_object()?;
        let kind = object.get("kind")?.as_str()?;
        let extra = |allowed: &[&str]| object.keys().any(|key| !allowed.contains(&key.as_str()));
        match kind {
            "owner" if !extra(&["kind"]) => Some(Self::Owner),
            "unknown" if !extra(&["kind"]) => Some(Self::Unknown),
            "person" if !extra(&["kind", "id"]) => object
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.trim().is_empty())
                .map(|id| Self::Person(id.to_owned())),
            _ => None,
        }
    }

    /// The tagged form [`LineSpeaker::from_tagged`] reads.
    pub fn tagged(&self) -> Value {
        match self {
            Self::Owner => serde_json::json!({"kind": "owner"}),
            Self::Person(id) => serde_json::json!({"kind": "person", "id": id}),
            Self::Unknown => serde_json::json!({"kind": "unknown"}),
        }
    }
}

/// One transcript line: its segment, its audio source and its statement
/// ordinal, the coordinate speaker label rows use for it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct OwnerLine {
    pub segment_dir: PathBuf,
    pub source: String,
    pub sentence_id: i64,
}

/// What a set wrote for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetLine {
    pub line: OwnerLine,
    /// The written entry's `set_at` token; a later conditional remove names it.
    pub set_at: String,
    /// The owner's entry this one replaced, if any.
    pub prior: Option<OwnerLineEntry>,
}

/// What a conditional remove found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ConditionalRemoval {
    /// The entry carried the given token and was removed.
    Removed,
    /// There was no entry for the line; nothing changed.
    Absent,
    /// The entry carries a different token, so it was left in place.
    Changed(OwnerLineEntry),
}

/// One prior entry to put back exactly as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRestore {
    pub segment_dir: PathBuf,
    pub entry: OwnerLineEntry,
}

/// What a restore did for one entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RestoreOutcome {
    /// The entry is back, with a merged-away speaker named by its survivor.
    Restored,
    /// The segment was deleted, so there is nothing to restore into.
    SegmentGone,
}

/// One line to put back once the owner withdraws an answer: only while the
/// line's entry still carries one of `tokens`, it becomes `restore`, or has
/// no entry when `restore` is `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineRevert {
    pub line: OwnerLine,
    pub tokens: BTreeSet<String>,
    pub restore: Option<OwnerLineEntry>,
}

/// What a revert did for one line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RevertOutcome {
    /// The entry carried one of the tokens and was put back.
    Reverted,
    /// The line has no entry, or one that carries another token; it was left.
    Left,
    /// The segment was deleted, so there is nothing to put back.
    SegmentGone,
}

/// Why an owner line label change was refused. A refused change writes nothing.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum OwnerLineLabelError {
    #[error("the segment can't take owner line labels: {0}")]
    SegmentUnavailable(String),
    #[error(
        "the segment's transcript views don't agree on one audio source (labels: {labelled:?}, transcript page: {transcript_page:?})"
    )]
    SourceUnsupported {
        labelled: Option<String>,
        transcript_page: Option<String>,
    },
    #[error("lines of {requested} can't be labelled; the segment's labelled source is {labelled}")]
    WrongSource { requested: String, labelled: String },
    #[error("no lines were named")]
    NoLines,
    #[error("line {sentence_id} isn't a line of {audio_source}")]
    LineNotFound {
        audio_source: String,
        sentence_id: i64,
    },
    #[error(
        "line {sentence_id} of {audio_source} is stored with sentence id {stored:?}, not its line number"
    )]
    LineIdMismatch {
        audio_source: String,
        sentence_id: i64,
        stored: Option<i64>,
    },
    #[error("person {0} wasn't found")]
    PersonNotFound(String),
    #[error("person {0} is hidden")]
    PersonHidden(String),
    #[error("{id} was merged into {successor}")]
    PersonMerged { id: String, successor: String },
    #[error("{0} isn't a person")]
    NotAPerson(String),
    #[error("the journal's owner identity isn't settled")]
    OwnerNotAdmitted,
    #[error("the journal's entities can't be read: {0}")]
    EntityStoreUnreadable(String),
    #[error("the segment's owner line labels can't be read: {0}")]
    FileUnreadable(String),
    #[error("an interrupted entity merge is waiting to be recovered")]
    MergeRecoveryPending,
    #[error("owner line labels are busy: {0}")]
    Lock(String),
    #[error("owner line labels couldn't be written: {0}")]
    Write(String),
}

impl OwnerLineLabelError {
    /// The stable reason code for this refusal.
    #[must_use]
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::SegmentUnavailable(_) => "owner_line_segment_unavailable",
            Self::SourceUnsupported { .. } => "owner_line_source_unsupported",
            Self::WrongSource { .. } => "owner_line_wrong_source",
            Self::NoLines => "owner_line_empty",
            Self::LineNotFound { .. } => "owner_line_not_found",
            Self::LineIdMismatch { .. } => "owner_line_id_mismatch",
            Self::PersonNotFound(_)
            | Self::PersonHidden(_)
            | Self::PersonMerged { .. }
            | Self::NotAPerson(_) => "owner_line_person_unavailable",
            Self::OwnerNotAdmitted => "owner_line_owner_not_admitted",
            Self::EntityStoreUnreadable(_) => "owner_line_people_unreadable",
            Self::FileUnreadable(_) => "owner_line_answers_unreadable",
            Self::MergeRecoveryPending => "owner_line_merge_recovery_pending",
            Self::Lock(_) => "owner_line_busy",
            Self::Write(_) => "owner_line_unsaved",
        }
    }
}

type Checkpoint<'a> = &'a dyn Fn() -> Result<(), String>;

/// Label `lines` as spoken by `speaker`, replacing any earlier answer for
/// each. Every segment and line is checked before any is written. Returns
/// one result per distinct line, ordered by line.
pub fn set_owner_line_labels(
    journal: &Path,
    lines: &[OwnerLine],
    speaker: &LineSpeaker,
) -> Result<Vec<SetLine>, OwnerLineLabelError> {
    set_owner_line_labels_checked(journal, lines, speaker, &|| Ok(()))
}

/// [`set_owner_line_labels`], renewing `checkpoint` before each step that
/// creates or replaces anything.
pub fn set_owner_line_labels_checked(
    journal: &Path,
    lines: &[OwnerLine],
    speaker: &LineSpeaker,
    checkpoint: Checkpoint<'_>,
) -> Result<Vec<SetLine>, OwnerLineLabelError> {
    set_locked(journal, lines, speaker, checkpoint).map(|(results, _, _)| results)
}

/// [`set_owner_line_labels`], then `then` while the answers are still locked.
/// When `then` fails, every owner file is put back to its exact prior bytes
/// and its error is returned inside `Ok`; the outer error is a refused set,
/// or a put-back that failed.
pub fn set_owner_line_labels_then<T, E>(
    journal: &Path,
    lines: &[OwnerLine],
    speaker: &LineSpeaker,
    then: impl FnOnce(&[SetLine]) -> Result<T, E>,
) -> Result<Result<T, E>, OwnerLineLabelError> {
    let (results, written, _trust) = set_locked(journal, lines, speaker, &|| Ok(()))?;
    finish(written, || then(&results))
}

/// Run `then` with `written` still locked, putting every written segment back
/// when it fails.
fn finish<T, E>(
    written: Vec<Written>,
    then: impl FnOnce() -> Result<T, E>,
) -> Result<Result<T, E>, OwnerLineLabelError> {
    match then() {
        Ok(value) => Ok(Ok(value)),
        Err(error) => match put_back(written) {
            Ok(()) => Ok(Err(error)),
            Err(failures) => Err(OwnerLineLabelError::Write(format!(
                "the change that went with these answers failed, and segments couldn't be put back: {failures}"
            ))),
        },
    }
}

type Locked = (
    Vec<SetLine>,
    Vec<Written>,
    solstone_core_entity::EntityTrustLock,
);

fn set_locked(
    journal: &Path,
    lines: &[OwnerLine],
    speaker: &LineSpeaker,
    checkpoint: Checkpoint<'_>,
) -> Result<Locked, OwnerLineLabelError> {
    if lines.is_empty() {
        return Err(OwnerLineLabelError::NoLines);
    }
    let trust = hold_trust(journal)?;
    let speaker = resolve_speaker(journal, speaker)?;
    let set_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Nanos, true);
    let mut changes = Vec::new();
    let mut results = Vec::new();
    for (segment_dir, keys) in by_segment(lines.iter().map(|line| {
        (
            line.segment_dir.as_path(),
            (line.source.as_str(), line.sentence_id),
        )
    })) {
        let segment = live_segment(segment_dir, checkpoint)?;
        let source = agreed_source(segment_dir)?;
        let statements = statements(&segment, &source)?;
        let prepared = prepare(segment, checkpoint)?;
        let mut next = prepared.entries.clone();
        for (requested, sentence_id) in keys {
            if requested != source {
                return Err(OwnerLineLabelError::WrongSource {
                    requested: requested.to_owned(),
                    labelled: source.clone(),
                });
            }
            let record =
                statements
                    .get(&sentence_id)
                    .ok_or_else(|| OwnerLineLabelError::LineNotFound {
                        audio_source: source.clone(),
                        sentence_id,
                    })?;
            let stored = record.get("sentence_id").and_then(Value::as_i64);
            if stored != Some(sentence_id) {
                return Err(OwnerLineLabelError::LineIdMismatch {
                    audio_source: source.clone(),
                    sentence_id,
                    stored,
                });
            }
            let entry = OwnerLineEntry {
                source: source.clone(),
                sentence_id,
                fingerprint: LineFingerprint::of_record(record),
                speaker: speaker.clone(),
                set_at: set_at.clone(),
            };
            let prior = find(&prepared.entries, entry.key()).cloned();
            upsert(&mut next, entry);
            results.push(SetLine {
                line: OwnerLine {
                    segment_dir: segment_dir.to_path_buf(),
                    source: source.clone(),
                    sentence_id,
                },
                set_at: set_at.clone(),
                prior,
            });
        }
        changes.push(Change { prepared, next });
    }
    let written = commit(changes, checkpoint)?;
    Ok((results, written, trust))
}

/// Remove the owner's answers for `lines`. A line with no answer, or a
/// segment with no owner line labels, is left exactly as it is and nothing is
/// created. Returns how many entries were removed.
pub fn remove_owner_line_labels(
    journal: &Path,
    lines: &[OwnerLine],
) -> Result<usize, OwnerLineLabelError> {
    remove_owner_line_labels_checked(journal, lines, &|| Ok(()))
}

/// [`remove_owner_line_labels`], renewing `checkpoint` before each step that
/// creates or replaces anything.
pub fn remove_owner_line_labels_checked(
    journal: &Path,
    lines: &[OwnerLine],
    checkpoint: Checkpoint<'_>,
) -> Result<usize, OwnerLineLabelError> {
    if lines.is_empty() {
        return Ok(0);
    }
    let _trust = hold_trust(journal)?;
    let mut changes = Vec::new();
    let mut removed = 0;
    for (segment_dir, keys) in by_segment(lines.iter().map(|line| {
        (
            line.segment_dir.as_path(),
            (line.source.as_str(), line.sentence_id),
        )
    })) {
        let prepared = prepare(live_segment(segment_dir, checkpoint)?, checkpoint)?;
        let next = prepared
            .entries
            .iter()
            .filter(|entry| !keys.contains(&entry.key()))
            .cloned()
            .collect::<Vec<_>>();
        removed += prepared.entries.len() - next.len();
        changes.push(Change { prepared, next });
    }
    commit(changes, checkpoint)?;
    Ok(removed)
}

/// Remove the owner's answer for `line` only while it still carries the
/// `set_at` token a set returned for it.
pub fn remove_owner_line_label_if_set_at(
    journal: &Path,
    line: &OwnerLine,
    set_at: &str,
) -> Result<ConditionalRemoval, OwnerLineLabelError> {
    let _trust = hold_trust(journal)?;
    let checkpoint: Checkpoint<'_> = &|| Ok(());
    let prepared = prepare(live_segment(&line.segment_dir, checkpoint)?, checkpoint)?;
    let key = (line.source.as_str(), line.sentence_id);
    match find(&prepared.entries, key) {
        None => Ok(ConditionalRemoval::Absent),
        Some(entry) if entry.set_at != set_at => Ok(ConditionalRemoval::Changed(entry.clone())),
        Some(_) => {
            let next = prepared
                .entries
                .iter()
                .filter(|entry| entry.key() != key)
                .cloned()
                .collect();
            commit(vec![Change { prepared, next }], checkpoint)?;
            Ok(ConditionalRemoval::Removed)
        }
    }
}

/// Put prior entries back exactly as they were, without checking the person
/// again. A speaker merged away since is written as the entity it was merged
/// into. An entry whose segment was deleted counts as restored. Returns one
/// outcome per entry, in the order given.
pub fn restore_owner_line_labels(
    journal: &Path,
    restores: &[LineRestore],
) -> Result<Vec<RestoreOutcome>, OwnerLineLabelError> {
    let _trust = hold_trust(journal)?;
    let checkpoint: Checkpoint<'_> = &|| Ok(());
    let mut outcomes = vec![RestoreOutcome::Restored; restores.len()];
    let mut groups = BTreeMap::<&Path, Vec<usize>>::new();
    for (index, restore) in restores.iter().enumerate() {
        groups
            .entry(restore.segment_dir.as_path())
            .or_default()
            .push(index);
    }
    let mut changes = Vec::new();
    for (segment_dir, indices) in groups {
        let Some(segment) = Segment::capture(segment_dir)? else {
            for index in indices {
                outcomes[index] = RestoreOutcome::SegmentGone;
            }
            continue;
        };
        segment
            .admitted(checkpoint)
            .map_err(OwnerLineLabelError::SegmentUnavailable)?;
        let prepared = prepare(segment, checkpoint)?;
        let mut next = prepared.entries.clone();
        for index in indices {
            let mut entry = restores[index].entry.clone();
            if let Some(id) = &entry.speaker {
                entry.speaker = Some(surviving_id(journal, id)?);
            }
            upsert(&mut next, entry);
        }
        changes.push(Change { prepared, next });
    }
    commit(changes, checkpoint)?;
    Ok(outcomes)
}

/// Put lines back once the owner withdraws an answer, each only while its
/// entry still carries one of its tokens. A restored entry is written exactly
/// as recorded, with a merged-away speaker named by its survivor, and the
/// person is not checked again. Every segment is checked before any is
/// written. Returns one outcome per revert, in the order given.
pub fn revert_owner_line_labels(
    journal: &Path,
    reverts: &[LineRevert],
) -> Result<Vec<RevertOutcome>, OwnerLineLabelError> {
    revert_owner_line_labels_then(journal, reverts, |outcomes| {
        Ok::<_, std::convert::Infallible>(outcomes.to_vec())
    })
    .map(|result| match result {
        Ok(outcomes) => outcomes,
        Err(never) => match never {},
    })
}

/// [`revert_owner_line_labels`], then `then` while the answers are still
/// locked, with the same put-back as [`set_owner_line_labels_then`].
pub fn revert_owner_line_labels_then<T, E>(
    journal: &Path,
    reverts: &[LineRevert],
    then: impl FnOnce(&[RevertOutcome]) -> Result<T, E>,
) -> Result<Result<T, E>, OwnerLineLabelError> {
    let _trust = hold_trust(journal)?;
    let checkpoint: Checkpoint<'_> = &|| Ok(());
    let mut outcomes = vec![RevertOutcome::Left; reverts.len()];
    let mut groups = BTreeMap::<&Path, Vec<usize>>::new();
    for (index, revert) in reverts.iter().enumerate() {
        groups
            .entry(revert.line.segment_dir.as_path())
            .or_default()
            .push(index);
    }
    let mut changes = Vec::new();
    for (segment_dir, indices) in groups {
        let Some(segment) = Segment::capture(segment_dir)? else {
            for index in indices {
                outcomes[index] = RevertOutcome::SegmentGone;
            }
            continue;
        };
        segment
            .admitted(checkpoint)
            .map_err(OwnerLineLabelError::SegmentUnavailable)?;
        let prepared = prepare(segment, checkpoint)?;
        let mut next = prepared.entries.clone();
        for index in indices {
            let revert = &reverts[index];
            let key = (revert.line.source.as_str(), revert.line.sentence_id);
            if !find(&next, key).is_some_and(|entry| revert.tokens.contains(&entry.set_at)) {
                continue;
            }
            next.retain(|entry| entry.key() != key);
            if let Some(restore) = &revert.restore {
                let mut entry = restore.clone();
                if let Some(id) = &entry.speaker {
                    entry.speaker = Some(surviving_id(journal, id)?);
                }
                upsert(&mut next, entry);
            }
            outcomes[index] = RevertOutcome::Reverted;
        }
        changes.push(Change { prepared, next });
    }
    let written = commit(changes, checkpoint)?;
    finish(written, || then(&outcomes))
}

/// A segment's owner answers as they stand, for showing whether an owner
/// correction still holds. A segment with none has none.
pub fn current_owner_line_entries(segment_dir: &Path) -> Result<Vec<OwnerLineEntry>, String> {
    match solstone_core_speaker_id::owner_line_labels::read_owner_line_labels(segment_dir) {
        solstone_core_speaker_id::owner_line_labels::OwnerLineLabels::Absent => Ok(Vec::new()),
        solstone_core_speaker_id::owner_line_labels::OwnerLineLabels::Present(entries) => {
            Ok(entries)
        }
        solstone_core_speaker_id::owner_line_labels::OwnerLineLabels::Unreadable(detail) => {
            Err(detail)
        }
    }
}

fn hold_trust(
    journal: &Path,
) -> Result<solstone_core_entity::EntityTrustLock, OwnerLineLabelError> {
    let trust = hold_entity_trust_lock(journal)
        .map_err(|error| OwnerLineLabelError::Lock(error.to_string()))?;
    // Recovery captured these files' bytes; changing them first would make
    // it refuse or restore over the owner's newer answer.
    if entity_merge_recovery_pending(journal) {
        return Err(OwnerLineLabelError::MergeRecoveryPending);
    }
    Ok(trust)
}

/// Group line keys by segment, dropping repeats.
fn by_segment<'a>(
    keys: impl Iterator<Item = (&'a Path, (&'a str, i64))>,
) -> BTreeMap<&'a Path, BTreeSet<(&'a str, i64)>> {
    let mut groups = BTreeMap::<&Path, BTreeSet<(&str, i64)>>::new();
    for (segment, key) in keys {
        groups.entry(segment).or_default().insert(key);
    }
    groups
}

fn find<'a>(entries: &'a [OwnerLineEntry], key: (&str, i64)) -> Option<&'a OwnerLineEntry> {
    entries.iter().find(|entry| entry.key() == key)
}

fn upsert(entries: &mut Vec<OwnerLineEntry>, entry: OwnerLineEntry) {
    match entries
        .iter_mut()
        .find(|existing| existing.key() == entry.key())
    {
        Some(existing) => *existing = entry,
        None => entries.push(entry),
    }
}

fn resolve_speaker(
    journal: &Path,
    speaker: &LineSpeaker,
) -> Result<Option<String>, OwnerLineLabelError> {
    let id = match speaker {
        LineSpeaker::Unknown => return Ok(None),
        LineSpeaker::Owner => None,
        LineSpeaker::Person(id) => Some(id),
    };
    // Listing reads a non-directory `entities` as an empty journal.
    let entities_dir = journal.join("entities");
    if std::fs::symlink_metadata(&entities_dir).is_ok_and(|metadata| !metadata.is_dir()) {
        return Err(OwnerLineLabelError::EntityStoreUnreadable(format!(
            "{} is not a directory",
            entities_dir.display()
        )));
    }
    let entities = load_all_journal_entities(journal)
        .map_err(|error| OwnerLineLabelError::EntityStoreUnreadable(error.to_string()))?;
    let admitted = || match admitted_owner_id(journal) {
        OwnerAdmission::Admitted(owner) => Ok(owner),
        OwnerAdmission::Invalid => Err(OwnerLineLabelError::OwnerNotAdmitted),
    };
    let Some(id) = id else {
        return admitted().map(Some);
    };
    match entities.into_iter().find(|entity| &entity.id == id) {
        Some(entity) if entity.is_blocked() => Err(OwnerLineLabelError::PersonHidden(id.clone())),
        Some(entity) if !is_admissible_person(&entity) => {
            Err(OwnerLineLabelError::NotAPerson(id.clone()))
        }
        Some(entity) if entity.is_principal() => match admitted()? {
            owner if &owner == id => Ok(Some(owner)),
            _ => Err(OwnerLineLabelError::OwnerNotAdmitted),
        },
        Some(_) => Ok(Some(id.clone())),
        None => {
            // The entity store reads a damaged identity as no entity; an
            // answer must not mistake that for a missing person.
            match observe_entity_identity(journal, id) {
                Ok(
                    IdentityObservation::Malformed(detail)
                    | IdentityObservation::Unreadable(detail),
                ) => {
                    return Err(OwnerLineLabelError::EntityStoreUnreadable(format!(
                        "entity {id} is damaged: {detail}"
                    )));
                }
                Ok(_) => {}
                Err(error) => {
                    return Err(OwnerLineLabelError::EntityStoreUnreadable(
                        error.to_string(),
                    ));
                }
            }
            match merged_away(journal, id).map_err(OwnerLineLabelError::EntityStoreUnreadable)? {
                Some(successor) => Err(OwnerLineLabelError::PersonMerged {
                    id: id.clone(),
                    successor,
                }),
                None => Err(OwnerLineLabelError::PersonNotFound(id.clone())),
            }
        }
    }
}

/// The live entity a stored id now names: itself, or the end of its merge chain.
fn surviving_id(journal: &Path, id: &str) -> Result<String, OwnerLineLabelError> {
    Ok(
        match merged_away(journal, id).map_err(OwnerLineLabelError::EntityStoreUnreadable)? {
            Some(successor) => live_merge_successor(journal, &successor).unwrap_or(successor),
            None => id.to_owned(),
        },
    )
}

/// The source both the transcript renderer and the transcript page label.
fn agreed_source(segment_dir: &Path) -> Result<String, OwnerLineLabelError> {
    let labelled = solstone_core_transcripts::labelled_audio_source(segment_dir);
    let (transcript_page, _) = solstone_core_transcripts::speaker_label_source(segment_dir);
    match (labelled, transcript_page) {
        (Some(labelled), Some(page)) if labelled == page => Ok(labelled),
        (labelled, transcript_page) => Err(OwnerLineLabelError::SourceUnsupported {
            labelled,
            transcript_page,
        }),
    }
}

/// A source's statements by ordinal.
fn statements(
    segment: &Segment,
    source: &str,
) -> Result<BTreeMap<i64, Map<String, Value>>, OwnerLineLabelError> {
    let bytes = segment
        .read(&format!("{source}.jsonl"))
        .map_err(OwnerLineLabelError::SegmentUnavailable)?
        .unwrap_or_default();
    let mut ordinal = 0;
    let mut statements = BTreeMap::new();
    for line in String::from_utf8_lossy(&bytes).lines() {
        if let Some(id) = solstone_core_transcripts::next_statement_ordinal(line, &mut ordinal)
            && let Ok(Value::Object(record)) = serde_json::from_str::<Value>(line.trim())
        {
            statements.insert(id, record);
        }
    }
    Ok(statements)
}

fn live_segment(
    segment_dir: &Path,
    checkpoint: Checkpoint<'_>,
) -> Result<Segment, OwnerLineLabelError> {
    let segment = Segment::capture(segment_dir)?.ok_or_else(|| {
        OwnerLineLabelError::SegmentUnavailable(format!(
            "{} was deleted or doesn't exist",
            segment_dir.display()
        ))
    })?;
    segment
        .admitted(checkpoint)
        .map_err(OwnerLineLabelError::SegmentUnavailable)?;
    Ok(segment)
}

/// A segment's current owner entries, read before any lock or creation.
struct Prepared {
    segment: Segment,
    talents: Option<Talents>,
    bytes: Option<Vec<u8>>,
    entries: Vec<OwnerLineEntry>,
}

fn prepare(segment: Segment, checkpoint: Checkpoint<'_>) -> Result<Prepared, OwnerLineLabelError> {
    let talents = segment
        .talents(false, checkpoint)
        .map_err(OwnerLineLabelError::SegmentUnavailable)?;
    let bytes = match &talents {
        Some(talents) => talents
            .read_store()
            .map_err(OwnerLineLabelError::FileUnreadable)?,
        None => None,
    };
    let entries = match &bytes {
        Some(bytes) => decode_owner_line_labels(bytes).map_err(|detail| {
            OwnerLineLabelError::FileUnreadable(format!(
                "{}: {detail}",
                segment
                    .path
                    .join("talents")
                    .join(OWNER_LINE_LABELS_FILE)
                    .display()
            ))
        })?,
        None => Vec::new(),
    };
    Ok(Prepared {
        segment,
        talents,
        bytes,
        entries,
    })
}

struct Change {
    prepared: Prepared,
    next: Vec<OwnerLineEntry>,
}

/// One segment written in this change, still locked, with its prior bytes.
struct Written {
    segment: Segment,
    talents: Talents,
    _lock: FileLock,
    prior: Option<Vec<u8>>,
}

fn commit(
    changes: Vec<Change>,
    checkpoint: Checkpoint<'_>,
) -> Result<Vec<Written>, OwnerLineLabelError> {
    let mut written = Vec::new();
    for change in changes {
        let mut next = change.next;
        let mut current = change.prepared.entries.clone();
        next.sort_by(|left, right| left.key().cmp(&right.key()));
        current.sort_by(|left, right| left.key().cmp(&right.key()));
        if next == current {
            continue;
        }
        match write_segment(change.prepared, &next, checkpoint) {
            Ok(segment) => written.push(segment),
            Err(error) => return Err(roll_back(written, error)),
        }
    }
    Ok(written)
}

fn write_segment(
    prepared: Prepared,
    next: &[OwnerLineEntry],
    checkpoint: Checkpoint<'_>,
) -> Result<Written, OwnerLineLabelError> {
    let Prepared {
        segment,
        talents,
        bytes,
        ..
    } = prepared;
    let talents = match talents {
        Some(talents) => talents,
        None => segment
            .talents(true, checkpoint)
            .map_err(OwnerLineLabelError::SegmentUnavailable)?
            .ok_or_else(|| OwnerLineLabelError::SegmentUnavailable("talents missing".into()))?,
    };
    let (lock, prior) = {
        let guarded = || {
            segment.admitted(checkpoint)?;
            talents.revalidate()
        };
        let lock = talents.lock(&guarded).map_err(OwnerLineLabelError::Lock)?;
        let current = talents
            .read_store()
            .map_err(OwnerLineLabelError::FileUnreadable)?;
        if current != bytes {
            return Err(OwnerLineLabelError::Write(
                "owner line labels changed while this change was being checked".to_owned(),
            ));
        }
        talents
            .write_store(&encode_owner_line_labels(next), &guarded)
            .map_err(OwnerLineLabelError::Write)?;
        (lock, current)
    };
    Ok(Written {
        segment,
        talents,
        _lock: lock,
        prior,
    })
}

/// Put back every segment already written, newest first, and report `error`.
fn roll_back(written: Vec<Written>, error: OwnerLineLabelError) -> OwnerLineLabelError {
    match put_back(written) {
        Ok(()) => error,
        Err(failures) => OwnerLineLabelError::Write(format!(
            "{error}; earlier segments couldn't be put back: {failures}"
        )),
    }
}

/// Put back every segment written, newest first, to its exact prior bytes.
fn put_back(written: Vec<Written>) -> Result<(), String> {
    let mut failures = Vec::new();
    for item in written.into_iter().rev() {
        let guarded = || {
            item.segment.admitted(&|| Ok(()))?;
            item.talents.revalidate()
        };
        let result = match &item.prior {
            Some(bytes) => item.talents.write_store(bytes, &guarded),
            None => item.talents.remove_store(),
        };
        if let Err(detail) = result {
            log::error!(
                "owner line labels in {} couldn't be put back: {detail}",
                item.segment.path.display()
            );
            failures.push(format!("{}: {detail}", item.segment.path.display()));
        }
    }
    if failures.is_empty() {
        Ok(())
    } else {
        Err(failures.join("; "))
    }
}

fn segment_is_live(path: &Path) -> Result<bool, String> {
    match owner_deletion_state(path) {
        Ok(OwnerDeletionState::Live) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

/// A captured segment directory.
struct Segment {
    path: PathBuf,
    #[cfg(unix)]
    root: solstone_core_journal_io::JournalRoot,
}

/// A segment's `talents/` directory, opened beneath its captured segment.
struct Talents {
    path: PathBuf,
    #[cfg(unix)]
    directory: solstone_core_journal_io::FlatDirectory,
}

impl Segment {
    /// Capture a live segment directory; `None` when it was deleted, is
    /// staged for removal or doesn't exist.
    fn capture(path: &Path) -> Result<Option<Self>, OwnerLineLabelError> {
        let unavailable = OwnerLineLabelError::SegmentUnavailable;
        if !segment_is_live(path).map_err(unavailable)?
            || !solstone_core_journal_io::path_lexists(path)
                .map_err(|error| unavailable(error.to_string()))?
        {
            return Ok(None);
        }
        Ok(Some(Self {
            path: path.to_path_buf(),
            #[cfg(unix)]
            root: solstone_core_journal_io::JournalRoot::open(path)
                .map_err(|error| unavailable(error.to_string()))?,
        }))
    }

    /// Renew the caller's authority and confirm the segment is still the
    /// captured, live directory.
    fn admitted(&self, checkpoint: Checkpoint<'_>) -> Result<(), String> {
        checkpoint()?;
        #[cfg(unix)]
        self.root
            .revalidate_canonical_binding()
            .map_err(|error| error.to_string())?;
        #[cfg(not(unix))]
        if !self.path.is_dir() {
            return Err(format!("{} is no longer a directory", self.path.display()));
        }
        if !segment_is_live(&self.path)? {
            return Err(format!("{} was deleted", self.path.display()));
        }
        Ok(())
    }

    fn read(&self, name: &str) -> Result<Option<Vec<u8>>, String> {
        #[cfg(unix)]
        return solstone_core_journal_io::read_bytes_bound(&self.root, std::ffi::OsStr::new(name))
            .map_err(|error| error.to_string());
        #[cfg(not(unix))]
        return read_optional(&self.path.join(name));
    }

    fn talents(&self, create: bool, checkpoint: Checkpoint<'_>) -> Result<Option<Talents>, String> {
        self.admitted(checkpoint)?;
        let path = self.path.join("talents");
        #[cfg(unix)]
        {
            let name = std::ffi::OsStr::new("talents");
            let directory = match solstone_core_journal_io::open_flat_directory_bound(
                &self.root, name, &self.path,
            )
            .map_err(|error| error.to_string())?
            {
                Some(directory) => directory,
                None if create => {
                    self.admitted(checkpoint)?;
                    solstone_core_journal_io::create_or_open_flat_directory_bound(
                        &self.root, name, 0o755, &self.path,
                    )
                    .map_err(|error| error.to_string())?
                }
                None => return Ok(None),
            };
            Ok(Some(Talents { path, directory }))
        }
        #[cfg(not(unix))]
        {
            if !path.is_dir() {
                if !create {
                    return Ok(None);
                }
                self.admitted(checkpoint)?;
                std::fs::create_dir(&path).map_err(|error| error.to_string())?;
            }
            Ok(Some(Talents { path }))
        }
    }
}

impl Talents {
    #[cfg(not(unix))]
    fn store_path(&self) -> PathBuf {
        self.path.join(OWNER_LINE_LABELS_FILE)
    }

    fn revalidate(&self) -> Result<(), String> {
        #[cfg(unix)]
        return self
            .directory
            .revalidate()
            .map_err(|error| error.to_string());
        #[cfg(not(unix))]
        return Ok(());
    }

    fn read_store(&self) -> Result<Option<Vec<u8>>, String> {
        #[cfg(unix)]
        return solstone_core_journal_io::read_bytes_bound(
            &self.directory,
            std::ffi::OsStr::new(OWNER_LINE_LABELS_FILE),
        )
        .map_err(|error| error.to_string());
        #[cfg(not(unix))]
        return read_optional(&self.store_path());
    }

    /// Hold the file's lock, created beneath the retained directory.
    fn lock(&self, admitted: Checkpoint<'_>) -> Result<FileLock, String> {
        #[cfg(unix)]
        return solstone_core_journal_io::hold_lock_bound_checked(
            &self.directory,
            std::ffi::OsStr::new(OWNER_LINE_LABELS_FILE),
            &self.path,
            LockOptions::default(),
            false,
            admitted,
        )
        .map_err(|error| error.to_string());
        #[cfg(not(unix))]
        return solstone_core_journal_io::hold_lock_checked(
            self.store_path(),
            LockOptions::default(),
            admitted,
        )
        .map_err(|error| error.to_string());
    }

    fn write_store(&self, bytes: &[u8], admitted: Checkpoint<'_>) -> Result<(), String> {
        #[cfg(unix)]
        {
            use solstone_core_journal_io::BoundAtomicOutcome;
            match solstone_core_journal_io::atomic_replace_bound_checked(
                &self.directory,
                std::ffi::OsStr::new(OWNER_LINE_LABELS_FILE),
                bytes,
                0o600,
                admitted,
            )
            .map_err(|error| error.to_string())?
            {
                BoundAtomicOutcome::Published { .. } => Ok(()),
                BoundAtomicOutcome::PublishedDurabilityUncertain { source, .. } => {
                    Err(format!("published; durability uncertain: {source}"))
                }
                BoundAtomicOutcome::PublishedObservationUncertain { source, .. } => {
                    Err(format!("published; observation uncertain: {source}"))
                }
            }
        }
        #[cfg(not(unix))]
        solstone_core_journal_io::atomic_replace_checked(
            self.store_path(),
            bytes,
            solstone_core_journal_io::AtomicWriteOptions { mode: Some(0o600) },
            admitted,
        )
        .map_err(|error| error.to_string())
    }

    /// Remove a file this change created, putting the segment back as it was.
    fn remove_store(&self) -> Result<(), String> {
        #[cfg(unix)]
        {
            let name = std::ffi::OsStr::new(OWNER_LINE_LABELS_FILE);
            match self.directory.observe(name).map_err(|e| e.to_string())? {
                Some(entry) => self
                    .directory
                    .remove_observed_entry(&entry)
                    .map_err(|error| error.to_string()),
                None => Ok(()),
            }
        }
        #[cfg(not(unix))]
        solstone_core_journal_io::remove_file(&self.path, OWNER_LINE_LABELS_FILE)
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

#[cfg(not(unix))]
fn read_optional(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.to_string()),
    }
}

#[cfg(test)]
#[path = "owner_line_labels_tests.rs"]
mod tests;
