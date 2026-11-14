// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The one route an owner removal takes, and the record it leaves.
//!
//! When something removes the owner's experience data kept in the journal or
//! its backup, because the owner acted or because of a policy the owner set,
//! it removes through a [`Removal`]. A removal is one operation: the owner's
//! confirmed act, named by what it was ([`Action`]) and where it came from
//! ([`Surface`]). Each thing it removed is one [`Unit`], and each unit removed
//! leaves one entry in the deletion record.
//!
//! # The record
//!
//! The record lives in the journal at [`RECORD_DIR`], one file per entry. An
//! entry is written once, after the removal it describes has happened, and is
//! never changed, pruned or removed. Because every entry has its own name and
//! nothing rewrites one, copying a backup or an archive over the journal can
//! only add entries.
//!
//! An entry holds no content: no transcript text, no media, no excerpts, and no
//! names the owner gave to what was removed. A recording is named by its time
//! and stream; an entity, facet or speaker is named only by its kind.
//!
//! The trash adds three [`Event`]s beside [`Event::Removed`]: a unit moved
//! to the trash, recovered from it, or erased from it. Larger [`Unit`]s (a
//! stretch of time) can come later. Readers skip an entry they do not
//! understand rather than fail.
//!
//! # The route
//!
//! [`Removal::commit`] is the way owner experience data leaves the journal. It
//! hands the caller a [`RemovalIo`], whose methods are the journal's contained
//! removal primitives, and records the unit only when the caller's removal
//! returns `Ok`. A removal that fails or is refused records nothing.
//!
//! Some removals are carried out by a mechanism of their own that already
//! guards its commit: retention's door (staging rename, tombstone, recovery) and
//! the backup tools. Those call [`Removal::record`] at their commit point, after
//! the removal has happened.
//!
//! The workspace lint configuration forbids the contained removal primitives
//! everywhere except this crate. Code that names one gets a lint failure that
//! points here.

#![allow(
    clippy::disallowed_methods,
    reason = "this crate is the route: the workspace-wide ban exists so that owner removals reach the primitives only through it"
)]

use std::fmt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::AtomicWriteOptions;
use solstone_core_journal_io::atomic::write_bytes_exclusive;
use solstone_core_journal_io::entry::Removed;
use solstone_core_journal_io::errors::AtomicWriteError;
use solstone_core_journal_io::errors::PathError;
use solstone_core_journal_io::paths::contained_path;

/// Where the deletion record lives, relative to the journal root.
///
/// Under `config/` because it is owner state: `health/` is derived state that
/// tooling wipes.
pub const RECORD_DIR: &str = "config/removals";

/// The entry format this build writes.
pub const SCHEMA: u32 = 1;

/// What happened to a unit.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Event {
    /// The unit is gone from the journal (or the backup), with no trash.
    Removed,
    /// The unit left the journal for the trash, where it can be recovered.
    Trashed,
    /// The unit came back from the trash, as it was.
    Recovered,
    /// The unit was erased from the trash.
    Erased,
}

impl Event {
    fn as_str(self) -> &'static str {
        match self {
            Self::Removed => "removed",
            Self::Trashed => "trashed",
            Self::Recovered => "recovered",
            Self::Erased => "erased",
        }
    }
}

/// What the owner did. One operation has one action.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    /// Deleted a recording (a whole segment).
    SegmentDelete,
    /// Deleted one workout and every piece of it.
    WorkoutDelete,
    /// Deleted one import and what it brought in.
    ImportDelete,
    /// Deleted an entity.
    EntityDelete,
    /// Removed one named entity's detections across a facet's days.
    DetectedEntitiesDelete,
    /// Deleted a facet and everything in it.
    FacetDelete,
    /// Merged a facet into another; files that collided are dropped.
    FacetMerge,
    /// Wiped what the journal knows about voices.
    SpeakersWipe,
    /// Deleted a memory an agent kept.
    MemoryDelete,
    /// Approved removing original recordings a policy proposed.
    OriginalsRelease,
    /// Let the voice-only setting remove original audio after transcription.
    VoiceOnlyRelease,
    /// Turned off the backup and deleted its copies.
    BackupOff,
    /// Removed segments or originals with the operator retention tool.
    RetentionTool,
    /// Finished removals that had stopped partway (Home's "finish them").
    FinishStopped,
    /// Deleted a stretch of time: everything recorded then, as one thing.
    StretchDelete,
    /// Recovered something from the trash.
    Recover,
    /// Erased something from the trash: a selection on the trash page, or
    /// the trash's own timer (surface [`Policy::Trash`]).
    Erase,
    /// Emptied the trash.
    EmptyTrash,
}

/// A page of the journal's web app.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Page {
    Transcripts,
    Body,
    Import,
    Entities,
    Settings,
    Home,
    Backup,
    Agents,
    Speakers,
}

/// A setting the owner turned on that removes things by itself.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Policy {
    /// "Leave out other voices": original audio goes once it is transcribed.
    VoiceOnly,
    /// The trash erases what has waited in it for the days the owner set.
    Trash,
}

/// Where a removal came from. Never "agents": a request that arrives from the
/// command line is [`Surface::Cli`].
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "surface", rename_all = "snake_case")]
pub enum Surface {
    /// A page of the journal's web app, Home and the backup page included.
    Web { page: Page },
    /// The `solstone` command line.
    Cli,
    /// A setting the owner turned on.
    Policy { policy: Policy },
}

/// The user agent prefix the `solstone` command line sends.
const CLI_USER_AGENT_PREFIX: &str = "solstone/";

impl Surface {
    /// The surface of a web request: the command line when the request came
    /// from the `solstone` client, otherwise `page`.
    pub fn of_request(page: Page, user_agent: Option<&str>) -> Self {
        match user_agent {
            Some(agent) if agent.starts_with(CLI_USER_AGENT_PREFIX) => Self::Cli,
            _ => Self::Web { page },
        }
    }
}

/// One thing a removal removed, in the owner's terms and without content.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Unit {
    /// A whole recording, by its day, stream and segment key.
    Segment {
        day: String,
        stream: String,
        segment: String,
    },
    /// An original recording file released from a segment that stays.
    Original {
        day: String,
        stream: String,
        segment: String,
        file: String,
    },
    /// The original recording files released together from one segment that
    /// stays, by their bare names: one item in the trash.
    Originals {
        day: String,
        stream: String,
        segment: String,
        files: Vec<String>,
    },
    /// A workout from a connected source, by the source's own id, and the day
    /// it began on, a date with no content, for the history's row.
    Workout {
        source: String,
        id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        day: Option<String>,
    },
    /// An import, by the journal's import id.
    Import { source: String, id: String },
    /// An entity. Its name is the owner's words, so it is not kept.
    Entity,
    /// One named entity's detections in a facet; the name is not kept.
    DetectedEntities,
    /// A facet and everything in it.
    Facet,
    /// A file dropped by a facet merge because the target had its own.
    FacetFile,
    /// Voiceprints, labels and corrections the journal kept about voices.
    Speakers,
    /// A memory an agent kept, by its day and segment key.
    Memory { day: String, segment: String },
    /// The copies held in a backup destination.
    Backup { storage: String },
}

/// One entry of the deletion record.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Entry {
    pub schema: u32,
    pub id: String,
    pub operation: String,
    pub event: Event,
    pub at: String,
    pub action: Action,
    #[serde(flatten)]
    pub surface: Surface,
    pub unit: Unit,
    /// The stretch of time the unit went or came back with, when it did: its
    /// bounds in the owner's wall time, no content.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub stretch: Option<Span>,
}

/// A stretch of time's bounds in the owner's wall time
/// (`YYYY-MM-DDTHH:MM:SS`).
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
pub struct Span {
    pub from: String,
    pub to: String,
}

/// Recording an entry failed. The removal it describes has already happened.
#[derive(Debug, thiserror::Error)]
pub enum RecordError {
    #[error("deletion record path: {0}")]
    Path(#[from] PathError),
    #[error("deletion record encode: {0}")]
    Encode(#[from] serde_json::Error),
    #[error("deletion record write: {0}")]
    Write(AtomicWriteError),
    #[error("deletion record entry {0} already holds a different removal")]
    Conflict(String),
}

/// An operation identifier: when it began, then 8 random hex digits.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OperationId(String);

impl OperationId {
    /// A new operation that began at `at`: for one owner act that a durable
    /// record carries across restarts and that spans several things, such as
    /// a selection recovered together.
    pub fn fresh(at: DateTime<Utc>) -> Self {
        let mut bytes = [0_u8; 4];
        // A failed random source still yields a unique id within this
        // process through the sequence, and the time prefix separates runs.
        if getrandom::fill(&mut bytes).is_err() {
            bytes = std::process::id().to_be_bytes();
        }
        Self(format!(
            "{}-{:02x}{:02x}{:02x}{:02x}",
            at.format("%Y%m%dT%H%M%S%3fZ"),
            bytes[0],
            bytes[1],
            bytes[2],
            bytes[3]
        ))
    }

    /// The id of an operation that a durable record carries across restarts,
    /// such as a held delete: the same record always yields the same id, so a
    /// resumed removal records its unit once.
    pub fn durable(requested_at: DateTime<Utc>, record_id: &str) -> Self {
        let safe: String = record_id
            .chars()
            .filter(|character| character.is_ascii_alphanumeric())
            .collect();
        Self(format!(
            "{}-{safe}",
            requested_at.format("%Y%m%dT%H%M%S%3fZ")
        ))
    }

    /// An id a durable record wrote down earlier, read back as it was.
    pub fn recorded(id: impl Into<String>) -> Self {
        Self(id.into())
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for OperationId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.0)
    }
}

/// One owner removal operation: the route every in-scope removal takes.
#[derive(Debug)]
pub struct Removal {
    journal_root: PathBuf,
    id: OperationId,
    action: Action,
    surface: Surface,
    sequence: AtomicU32,
    /// Whether a durable record carries this operation across restarts.
    durable: bool,
    /// The one thing of several in this operation that these entries are
    /// about, when the operation spans more than one.
    item: Option<String>,
    /// The stretch of time these entries' units belong to.
    stretch: Option<Span>,
}

impl Removal {
    /// Begin an operation now.
    pub fn begin(journal_root: &Path, action: Action, surface: Surface) -> Self {
        let mut removal = Self::with_id(
            journal_root,
            OperationId::fresh(Utc::now()),
            action,
            surface,
        );
        removal.durable = false;
        removal
    }

    /// Continue an operation whose id a durable record carries.
    pub fn with_id(journal_root: &Path, id: OperationId, action: Action, surface: Surface) -> Self {
        Self {
            journal_root: journal_root.to_path_buf(),
            id,
            action,
            surface,
            sequence: AtomicU32::new(0),
            durable: true,
            item: None,
            stretch: None,
        }
    }

    /// Record this operation's entries as part of a stretch of time.
    #[must_use]
    pub fn in_stretch(mut self, stretch: Option<Span>) -> Self {
        self.stretch = stretch;
        self
    }

    /// Name this operation's entries by `item` too: for a durable operation
    /// that spans several things, whose units can read the same (two
    /// entities, two facets), so each thing's entry has its own name and is
    /// still found again when its step is resumed.
    #[must_use]
    pub fn for_item(mut self, item: &str) -> Self {
        self.item = Some(
            item.chars()
                .filter(|character| character.is_ascii_alphanumeric() || *character == '-')
                .collect(),
        );
        self
    }

    pub fn id(&self) -> &OperationId {
        &self.id
    }

    pub fn action(&self) -> Action {
        self.action
    }

    pub fn surface(&self) -> Surface {
        self.surface
    }

    pub fn journal_root(&self) -> &Path {
        &self.journal_root
    }

    /// Run `remove` with the journal's removal primitives and record `unit`
    /// when it returns `Ok`. On `Err` nothing is recorded.
    ///
    /// A record that cannot be written is logged, not returned: the removal
    /// has happened, and the owner's outcome must not change because its
    /// record failed.
    pub fn commit<T, E>(
        &self,
        unit: Unit,
        remove: impl FnOnce(&RemovalIo<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let io = RemovalIo { removal: self };
        let value = remove(&io)?;
        self.record(unit);
        Ok(value)
    }

    /// As [`Removal::commit`], recording `event` rather than
    /// [`Event::Removed`]: the trash erasing what it held.
    pub fn commit_event<T, E>(
        &self,
        event: Event,
        unit: Unit,
        remove: impl FnOnce(&RemovalIo<'_>) -> Result<T, E>,
    ) -> Result<T, E> {
        let io = RemovalIo { removal: self };
        let value = remove(&io)?;
        self.record_event(event, unit);
        Ok(value)
    }

    /// As [`Removal::commit`], for a removal that may find nothing to remove:
    /// `remove` returns its value and whether it removed anything, and the
    /// unit is recorded only when it did.
    pub fn commit_when<T, E>(
        &self,
        unit: Unit,
        remove: impl FnOnce(&RemovalIo<'_>) -> Result<(T, bool), E>,
    ) -> Result<T, E> {
        let io = RemovalIo { removal: self };
        let (value, removed) = remove(&io)?;
        if removed {
            self.record(unit);
        }
        Ok(value)
    }

    /// Record a unit that a guarded mechanism of its own (retention's door,
    /// the backup tools) has already removed. Call it only after the removal
    /// has happened. Failures are logged, as in [`Removal::commit`].
    pub fn record(&self, unit: Unit) {
        if let Err(error) = self.try_record(unit) {
            log::error!(
                "deletion record for operation {} was not written: {error}",
                self.id
            );
        }
    }

    /// Record that `unit` moved to, came back from, or was erased from the
    /// trash (or was removed), logging a failure as [`Removal::record`] does.
    pub fn record_event(&self, event: Event, unit: Unit) {
        if let Err(error) = self.try_record_event(event, unit) {
            log::error!(
                "deletion record for operation {} was not written: {error}",
                self.id
            );
        }
    }

    /// Record a unit and report a failure to write it.
    pub fn try_record(&self, unit: Unit) -> Result<(), RecordError> {
        self.try_record_event(Event::Removed, unit)
    }

    /// Record one event for a unit and report a failure to write it.
    pub fn try_record_event(&self, event: Event, unit: Unit) -> Result<(), RecordError> {
        // A durable operation can be run again after a restart, in any order,
        // so its entries are named by their unit: a unit already recorded is
        // found again whatever else the run did first. A fresh operation runs
        // once, so its entries are numbered, which also keeps two units that
        // read the same (two files a merge dropped) as two entries.
        let id = if self.durable {
            // Stage 1's entries keep their names; the trash's events carry
            // theirs, so one operation can record a unit trashed and erased.
            // An operation over several things names each thing too, so two
            // whose units read the same are two entries. Names already
            // written stay as they are: entries are never renamed.
            match (event, &self.item) {
                (event, Some(item)) => format!(
                    "{}-{}-{item}-u{}",
                    self.id,
                    event.as_str(),
                    unit_digest(&unit)?
                ),
                (Event::Removed, None) => format!("{}-u{}", self.id, unit_digest(&unit)?),
                (event, None) => {
                    format!("{}-{}-u{}", self.id, event.as_str(), unit_digest(&unit)?)
                }
            }
        } else {
            let sequence = self.sequence.fetch_add(1, Ordering::SeqCst);
            format!("{}-{sequence:04}", self.id)
        };
        let entry = Entry {
            schema: SCHEMA,
            id,
            operation: self.id.to_string(),
            event,
            at: Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true),
            action: self.action,
            surface: self.surface,
            unit,
            stretch: self.stretch.clone(),
        };
        write_entry(&self.journal_root, &entry)
    }
}

/// The journal's contained removal primitives, reachable inside
/// [`Removal::commit`].
pub struct RemovalIo<'a> {
    removal: &'a Removal,
}

impl RemovalIo<'_> {
    /// The journal this removal acts on.
    pub fn journal_root(&self) -> &Path {
        &self.removal.journal_root
    }

    /// Remove one file below the journal root.
    pub fn remove_file(&self, rel: &str) -> Result<Removed, PathError> {
        solstone_core_journal_io::entry::remove_file(&self.removal.journal_root, rel)
    }

    /// Remove a directory tree below the journal root; absent is removed.
    pub fn remove_tree(&self, rel: &str) -> Result<(), PathError> {
        solstone_core_journal_io::removal::remove_dir_all(&self.removal.journal_root, rel)
    }

    /// Remove an already-resolved directory tree contained in the journal.
    pub fn remove_contained_tree(&self, path: &Path) -> Result<(), PathError> {
        solstone_core_journal_io::removal::remove_contained_tree(&self.removal.journal_root, path)
    }
}

fn unit_digest(unit: &Unit) -> Result<String, RecordError> {
    let digest = Sha256::digest(serde_json::to_vec(unit)?);
    Ok(digest
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect())
}

fn entry_rel(id: &str) -> String {
    format!("{RECORD_DIR}/{id}.json")
}

fn write_entry(journal_root: &Path, entry: &Entry) -> Result<(), RecordError> {
    let rel = entry_rel(&entry.id);
    let path = contained_path(journal_root, &rel)?;
    let bytes = serde_json::to_vec_pretty(entry)?;
    match write_bytes_exclusive(&path, &bytes, AtomicWriteOptions { mode: Some(0o600) }) {
        Ok(()) => Ok(()),
        // A resumed removal recording its unit again: the first entry stands.
        // A different unit under the same name is reported, never dropped
        // silently.
        Err(AtomicWriteError::Io { source, .. })
            if source.kind() == std::io::ErrorKind::AlreadyExists =>
        {
            let existing = std::fs::read(&path)
                .ok()
                .and_then(|bytes| serde_json::from_slice::<Entry>(&bytes).ok());
            match existing {
                Some(existing)
                    if existing.unit == entry.unit
                        && existing.event == entry.event
                        && existing.action == entry.action
                        && existing.operation == entry.operation =>
                {
                    Ok(())
                }
                _ => Err(RecordError::Conflict(rel)),
            }
        }
        Err(error) => Err(RecordError::Write(error)),
    }
}

/// Reading the deletion record failed.
#[derive(Debug, thiserror::Error)]
pub enum HistoryError {
    #[error("deletion record path: {0}")]
    Path(#[from] PathError),
    #[error("deletion record read {path}: {source}")]
    Read {
        path: PathBuf,
        source: std::io::Error,
    },
}

/// The newest `limit` entries of the deletion record, newest first, and how
/// many entry files the record holds in all.
///
/// The record is never pruned, so only the newest names are read: an entry's
/// name begins with its operation's start time. An entry this build cannot
/// read (a later format, a damaged file) is skipped and logged, still counted,
/// and never removed.
pub fn history(journal_root: &Path, limit: usize) -> Result<History, HistoryError> {
    let dir = contained_path(journal_root, RECORD_DIR)?;
    let listing = match std::fs::read_dir(&dir) {
        Ok(listing) => listing,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(History::default());
        }
        Err(source) => return Err(HistoryError::Read { path: dir, source }),
    };
    let mut names = Vec::new();
    for item in listing {
        let item = item.map_err(|source| HistoryError::Read {
            path: dir.clone(),
            source,
        })?;
        let name = item.file_name();
        if let Some(name) = name.to_str().filter(|name| name.ends_with(".json")) {
            names.push(name.to_owned());
        }
    }
    let total = names.len();
    names.sort_unstable_by(|left, right| right.cmp(left));
    let mut entries = Vec::new();
    for name in names {
        if entries.len() >= limit {
            break;
        }
        let path = dir.join(&name);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) => {
                log::warn!(
                    "deletion record entry {} unreadable: {error}",
                    path.display()
                );
                continue;
            }
        };
        match serde_json::from_slice::<Entry>(&bytes) {
            Ok(entry) => entries.push(entry),
            Err(error) => {
                log::warn!("deletion record entry {} skipped: {error}", path.display());
            }
        }
    }
    entries.sort_by(|left, right| right.at.cmp(&left.at).then(right.id.cmp(&left.id)));
    Ok(History { total, entries })
}

/// What [`history`] returns.
#[derive(Clone, Debug, Default, Serialize)]
pub struct History {
    pub total: usize,
    pub entries: Vec<Entry>,
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    reason = "test code"
)]
mod tests {
    use super::*;

    struct Journal(PathBuf);

    impl Journal {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "solstone-removals-{name}-{}-{}",
                std::process::id(),
                OperationId::fresh(Utc::now())
            ));
            std::fs::create_dir_all(&root).unwrap();
            Self(root)
        }
    }

    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    fn segment(key: &str) -> Unit {
        Unit::Segment {
            day: "20261108".to_owned(),
            stream: "field.audio".to_owned(),
            segment: key.to_owned(),
        }
    }

    #[test]
    fn a_committed_removal_removes_and_records_one_entry_without_content() {
        let journal = Journal::new("commit");
        let rel = "chronicle/20261108/field.audio/070000_300";
        std::fs::create_dir_all(journal.0.join(rel)).unwrap();
        std::fs::write(journal.0.join(rel).join("audio.jsonl"), b"secret words").unwrap();

        let removal = Removal::begin(
            &journal.0,
            Action::SegmentDelete,
            Surface::Web {
                page: Page::Transcripts,
            },
        );
        removal
            .commit(segment("070000_300"), |io| io.remove_tree(rel))
            .unwrap();

        assert!(!journal.0.join(rel).exists());
        let history = history(&journal.0, 10).unwrap();
        assert_eq!(history.total, 1);
        let entry = &history.entries[0];
        assert_eq!(entry.event, Event::Removed);
        assert_eq!(entry.operation, removal.id().as_str());
        assert_eq!(entry.unit, segment("070000_300"));
        let raw = std::fs::read_to_string(journal.0.join(entry_rel(&entry.id))).unwrap();
        assert!(!raw.contains("secret"));
        assert!(raw.contains("\"surface\": \"web\""));
        assert!(raw.contains("\"page\": \"transcripts\""));
    }

    #[test]
    fn a_failed_or_refused_removal_records_nothing() {
        let journal = Journal::new("refused");
        let removal = Removal::begin(&journal.0, Action::FacetDelete, Surface::Cli);
        let refused: Result<(), &str> = removal.commit(Unit::Facet, |_| Err("last facet"));
        assert!(refused.is_err());
        assert_eq!(history(&journal.0, 10).unwrap().total, 0);
    }

    #[test]
    fn a_resumed_operation_records_its_unit_once() {
        let journal = Journal::new("resume");
        let requested = Utc::now();
        for _ in 0..2 {
            let removal = Removal::with_id(
                &journal.0,
                OperationId::durable(requested, "0123456789abcdef0123456789abcdef"),
                Action::SegmentDelete,
                Surface::Cli,
            );
            removal.record(segment("070000_300"));
        }
        assert_eq!(history(&journal.0, 10).unwrap().total, 1);
    }

    #[test]
    fn copying_an_older_record_over_a_newer_one_only_adds() {
        // A restore writes the backup's files over the live journal. Entries
        // have unique names and are never rewritten, so whatever the backup
        // holds can only add to what the journal holds.
        let live = Journal::new("live");
        let backup = Journal::new("backup");
        let older = Removal::begin(&backup.0, Action::EntityDelete, Surface::Cli);
        older.record(Unit::Entity);
        std::fs::create_dir_all(live.0.join(RECORD_DIR)).unwrap();
        for item in std::fs::read_dir(backup.0.join(RECORD_DIR)).unwrap() {
            let item = item.unwrap();
            std::fs::copy(item.path(), live.0.join(RECORD_DIR).join(item.file_name())).unwrap();
        }
        let newer = Removal::begin(&live.0, Action::FacetDelete, Surface::Cli);
        newer.record(Unit::Facet);
        let before = history(&live.0, 10).unwrap();
        for item in std::fs::read_dir(backup.0.join(RECORD_DIR)).unwrap() {
            let item = item.unwrap();
            std::fs::copy(item.path(), live.0.join(RECORD_DIR).join(item.file_name())).unwrap();
        }
        let after = history(&live.0, 10).unwrap();
        assert_eq!(before.total, 2);
        assert_eq!(after.entries, before.entries);
    }

    #[test]
    fn an_entry_from_a_later_format_is_skipped_not_fatal() {
        let journal = Journal::new("later");
        Removal::begin(&journal.0, Action::FacetDelete, Surface::Cli).record(Unit::Facet);
        std::fs::write(
            journal
                .0
                .join(RECORD_DIR)
                .join("99999999T000000000Z-ffffffff-0000.json"),
            br#"{"schema":2,"event":"erased","unit":{"kind":"time_range"}}"#,
        )
        .unwrap();
        let history = history(&journal.0, 10).unwrap();
        assert_eq!(history.total, 2, "an unreadable entry is still counted");
        assert_eq!(history.entries.len(), 1);
    }

    #[test]
    fn a_resumed_operation_finds_each_unit_again_in_any_order() {
        let journal = Journal::new("reorder");
        let requested = Utc::now();
        let durable = || {
            Removal::with_id(
                &journal.0,
                OperationId::durable(requested, "0123456789abcdef0123456789abcdef"),
                Action::WorkoutDelete,
                Surface::Cli,
            )
        };
        durable().record(segment("070000_300"));
        let resumed = durable();
        resumed.record(segment("080000_300"));
        resumed.record(segment("070000_300"));
        assert_eq!(history(&journal.0, 10).unwrap().total, 2);
    }

    #[test]
    fn an_operation_over_several_things_records_each_once() {
        // Two entities read the same as units: only the item tells them
        // apart, so each is named by its item and a resumed one is found.
        let journal = Journal::new("several");
        let operation = OperationId::fresh(Utc::now());
        let removal = |item: &str| {
            Removal::with_id(
                &journal.0,
                operation.clone(),
                Action::EntityDelete,
                Surface::Cli,
            )
            .for_item(item)
        };
        removal("20261111T000000000Z-aaaaaaaa").record_event(Event::Trashed, Unit::Entity);
        removal("20261111T000000000Z-bbbbbbbb").record_event(Event::Trashed, Unit::Entity);
        // The first one's step resumed after a stop.
        removal("20261111T000000000Z-aaaaaaaa").record_event(Event::Trashed, Unit::Entity);
        let history = history(&journal.0, 10).unwrap();
        assert_eq!(history.total, 2);
        assert!(
            history
                .entries
                .iter()
                .all(|entry| entry.operation == operation.as_str())
        );
    }

    #[test]
    fn names_written_without_an_item_still_read_beside_those_with_one() {
        let journal = Journal::new("names");
        let requested = Utc::now();
        Removal::with_id(
            &journal.0,
            OperationId::durable(requested, "20261111T000000000Z-aaaaaaaa"),
            Action::EntityDelete,
            Surface::Cli,
        )
        .record_event(Event::Trashed, Unit::Entity);
        Removal::with_id(
            &journal.0,
            OperationId::fresh(requested),
            Action::EntityDelete,
            Surface::Cli,
        )
        .for_item("20261111T000000000Z-bbbbbbbb")
        .record_event(Event::Trashed, Unit::Entity);
        let history = history(&journal.0, 10).unwrap();
        assert_eq!(history.total, 2);
        assert_eq!(history.entries.len(), 2);
    }

    #[test]
    fn a_different_removal_under_a_taken_name_is_reported() {
        let journal = Journal::new("conflict");
        let first = Removal::begin(&journal.0, Action::FacetDelete, Surface::Cli);
        first.try_record(Unit::Facet).unwrap();
        let mut clash = Removal::with_id(
            &journal.0,
            first.id().clone(),
            Action::EntityDelete,
            Surface::Cli,
        );
        clash.durable = false;
        assert!(matches!(
            clash.try_record(Unit::Entity),
            Err(RecordError::Conflict(_))
        ));
        assert_eq!(history(&journal.0, 10).unwrap().total, 1);
    }

    #[test]
    fn the_command_line_is_named_as_the_command_line() {
        assert_eq!(
            Surface::of_request(Page::Settings, Some("solstone/2.0.72")),
            Surface::Cli
        );
        assert_eq!(
            Surface::of_request(Page::Settings, Some("Mozilla/5.0")),
            Surface::Web {
                page: Page::Settings
            }
        );
        assert_eq!(
            Surface::of_request(Page::Settings, None),
            Surface::Web {
                page: Page::Settings
            }
        );
    }

    #[test]
    fn history_is_newest_first_and_bounded() {
        let journal = Journal::new("order");
        let removal = Removal::begin(&journal.0, Action::SegmentDelete, Surface::Cli);
        for key in ["070000_300", "070500_300", "071000_300"] {
            removal.record(segment(key));
        }
        let history = history(&journal.0, 2).unwrap();
        assert_eq!(history.total, 3);
        assert_eq!(history.entries.len(), 2);
        assert_eq!(history.entries[0].unit, segment("071000_300"));
    }
}
