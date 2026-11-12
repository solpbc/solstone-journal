// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What the calendar source has taken in, so a resend adds nothing.
//!
//! A phone sends its calendar again whenever its calendar source is set up
//! again: after it is paired to a journal, or turned back on. The journal may
//! already hold those events, sent before from this phone or another. Each
//! note's bytes can differ on a resend (when the phone noticed the change, the
//! zone it was in, which other events share the file), so the segment's own
//! byte-identical check does not catch it.
//!
//! The source keeps a record of every event it has taken in, keyed by the
//! event's identity, in `config/calendar/received/`: what the latest note said
//! about it, apart from when it was noticed. An upload's note that says the
//! same about an event is dropped before anything is written; a file with
//! nothing left is answered as already held, and an upload with nothing left
//! writes no segment. Like the screenshots record, it is the source's
//! business, not the trash's: it survives a segment's erase, so a resend never
//! brings back an event the owner deleted.
//!
//! A journal that held calendar notes before the record existed builds it once,
//! from the calendar files in its calendar streams. The record is committed
//! only after what is new is durable: a crash in between drops the same notes
//! on the resend, and the segment's own duplicate check then answers it.

use std::collections::{BTreeMap, HashMap};
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};
use solstone_core_callosum::FileDescriptor;
use solstone_core_format::calendar::{
    CALENDAR_SOURCE, CalendarNote, calendar_file_name, is_calendar_file_name, parse_calendar_note,
    read_calendar_notes,
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace, create_directory_with_mode};

use crate::model::IncomingFile;

const RECORD_DIR: &str = "config/calendar/received";

/// Present once the record covers every calendar note the journal held when it
/// was first made.
const BUILT_MARKER: &str = "built";

const PRIVATE: AtomicWriteOptions = AtomicWriteOptions { mode: Some(0o600) };

/// The latest note the journal took in for one event.
#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
struct Held {
    /// What the note says about the event, apart from when it was noticed.
    content: String,
    changed_at_ms: i64,
    rank: i64,
}

impl Held {
    fn of(note: &CalendarNote) -> Self {
        Self {
            content: content_of(note),
            changed_at_ms: note.changed_at_ms(),
            rank: note.change.rank(),
        }
    }

    /// Whether this note is at least as late as `other`, in the order the
    /// index picks an event's latest note by.
    fn supersedes(&self, other: &Held) -> bool {
        (self.changed_at_ms, self.rank) >= (other.changed_at_ms, other.rank)
    }
}

type Bucket = BTreeMap<String, Held>;

/// How the answer names files the journal did not write as sent.
#[derive(Clone, Debug, Default)]
pub(crate) struct Receipt {
    /// Files every note of which the journal already holds.
    held: Vec<FileDescriptor>,
    /// Files written with only their new notes, by the name they were written
    /// as: the name, size and SHA-256 they arrived with.
    trimmed: HashMap<String, (String, u64, String)>,
}

impl Receipt {
    /// The answer's descriptors, naming each file as it arrived.
    pub(crate) fn restore(&self, descriptors: Vec<FileDescriptor>) -> Vec<FileDescriptor> {
        let mut restored = descriptors
            .into_iter()
            .map(|mut descriptor| {
                if let Some((submitted, size, sha256)) = self.trimmed.get(&descriptor.submitted) {
                    descriptor.submitted = submitted.clone();
                    descriptor.size = *size;
                    descriptor.sha256 = sha256.clone();
                }
                descriptor
            })
            .collect::<Vec<_>>();
        restored.extend(self.held.iter().cloned());
        restored
    }

    pub(crate) fn held(&self) -> &[FileDescriptor] {
        &self.held
    }
}

/// An upload with what the journal already holds taken out.
pub(crate) struct Prepared {
    /// The files to write, each with only its new notes.
    pub(crate) files: Vec<IncomingFile>,
    pub(crate) receipt: Receipt,
    updates: BTreeMap<String, Bucket>,
    built: bool,
}

impl Prepared {
    /// Remember what this upload brought in. Called once it is durable.
    pub(crate) fn commit(&self, journal: &Path) -> io::Result<()> {
        let dir = journal.join(RECORD_DIR);
        if !self.updates.is_empty() || self.built {
            if let Some(parent) = dir.parent() {
                create_directory_with_mode(parent, 0o700).map_err(io::Error::other)?;
            }
            create_directory_with_mode(&dir, 0o700).map_err(io::Error::other)?;
        }
        for (name, bucket) in &self.updates {
            let mut bytes = serde_json::to_vec(bucket).map_err(io::Error::other)?;
            bytes.push(b'\n');
            atomic_replace(bucket_path(journal, name), &bytes, PRIVATE)
                .map_err(io::Error::other)?;
        }
        if self.built {
            atomic_replace(dir.join(BUILT_MARKER), b"1\n", PRIVATE).map_err(io::Error::other)?;
        }
        Ok(())
    }
}

/// The record's view for one upload: buckets read so far, with this upload's
/// changes on top.
struct View<'a> {
    journal: &'a Path,
    buckets: HashMap<String, Bucket>,
    touched: BTreeMap<String, ()>,
}

impl View<'_> {
    fn bucket(&mut self, name: &str) -> io::Result<&mut Bucket> {
        if !self.buckets.contains_key(name) {
            let bucket = read_bucket(self.journal, name)?;
            self.buckets.insert(name.to_owned(), bucket);
        }
        Ok(self.buckets.get_mut(name).expect("bucket just read"))
    }

    fn get(&mut self, key: &str) -> io::Result<Option<Held>> {
        Ok(self.bucket(&key[..2])?.get(key).cloned())
    }

    /// Keep `held` for `key` if it is the event's latest note.
    fn offer(&mut self, key: String, held: Held) -> io::Result<()> {
        let name = key[..2].to_owned();
        let bucket = self.bucket(&name)?;
        let newer = bucket
            .get(&key)
            .is_none_or(|current| held.supersedes(current));
        if newer && bucket.get(&key) != Some(&held) {
            bucket.insert(key, held);
            self.touched.insert(name, ());
        }
        Ok(())
    }

    fn into_updates(mut self) -> BTreeMap<String, Bucket> {
        self.touched
            .into_keys()
            .filter_map(|name| self.buckets.remove(&name).map(|bucket| (name, bucket)))
            .collect()
    }
}

/// Take out of each file the notes that say what the journal already holds.
///
/// A record that can't be read refuses the upload rather than reading as
/// empty: an empty reading would bring back every event the owner deleted.
pub(crate) fn prepare(journal: &Path, files: Vec<IncomingFile>) -> io::Result<Prepared> {
    let mut view = View {
        journal,
        buckets: HashMap::new(),
        touched: BTreeMap::new(),
    };
    let built = !journal.join(RECORD_DIR).join(BUILT_MARKER).is_file();
    if built {
        build(journal, &mut view)?;
    }
    let mut kept_files = Vec::new();
    let mut receipt = Receipt::default();
    for file in files {
        let text = String::from_utf8_lossy(&file.bytes).into_owned();
        let mut kept = String::new();
        let mut dropped = 0usize;
        for line in text.lines().filter(|line| !line.trim().is_empty()) {
            // Ingest checked every line already; one that does not read is
            // kept as sent, never dropped.
            let Ok(note) = parse_calendar_note(line) else {
                kept.push_str(line);
                kept.push('\n');
                continue;
            };
            let key = identity_key(&note);
            let held = Held::of(&note);
            if view
                .get(&key)?
                .is_some_and(|current| current.content == held.content)
            {
                dropped += 1;
                continue;
            }
            view.offer(key, held)?;
            kept.push_str(line);
            kept.push('\n');
        }
        let size = file.bytes.len() as u64;
        let sha256 = format!("{:x}", Sha256::digest(&file.bytes));
        if dropped == 0 {
            kept_files.push(file);
        } else if kept.is_empty() {
            let mut extra = file.descriptor_extra.clone();
            extra.insert(
                "disposition".to_owned(),
                Value::String("already_held".to_owned()),
            );
            receipt.held.push(FileDescriptor {
                submitted: file.submitted.clone(),
                written: file.submitted,
                size,
                sha256,
                extra,
            });
        } else {
            let bytes = kept.into_bytes();
            let name = calendar_file_name(&bytes);
            receipt
                .trimmed
                .insert(name.clone(), (file.submitted, size, sha256));
            kept_files.push(IncomingFile {
                submitted: name,
                bytes,
                descriptor_extra: file.descriptor_extra,
            });
        }
    }
    Ok(Prepared {
        files: kept_files,
        receipt,
        updates: view.into_updates(),
        built,
    })
}

/// Record every note in the journal's calendar streams, for a journal that
/// held calendar notes before the record existed.
fn build(journal: &Path, view: &mut View<'_>) -> io::Result<()> {
    let streams = solstone_core_segment::list_stream_bindings(journal)
        .map_err(io::Error::other)?
        .into_iter()
        .filter(|binding| binding.source == CALENDAR_SOURCE)
        .map(|binding| binding.name)
        .collect::<Vec<_>>();
    if streams.is_empty() {
        return Ok(());
    }
    let days = solstone_core_journal_io::day_dirs(journal).map_err(io::Error::other)?;
    for day in days.values() {
        for stream in &streams {
            let Some(segments) = read_dir_if_present(&day.join(stream))? else {
                continue;
            };
            for segment in segments {
                let segment = segment?.path();
                let Some(entries) = read_dir_if_present(&segment)? else {
                    continue;
                };
                for entry in entries {
                    let entry = entry?;
                    let name = entry.file_name();
                    if !name.to_str().is_some_and(is_calendar_file_name) {
                        continue;
                    }
                    let text = std::fs::read_to_string(entry.path())?;
                    let (notes, _warnings) = read_calendar_notes(&text);
                    for (_, note) in notes {
                        view.offer(identity_key(&note), Held::of(&note))?;
                    }
                }
            }
        }
    }
    Ok(())
}

fn read_dir_if_present(path: &Path) -> io::Result<Option<std::fs::ReadDir>> {
    match std::fs::read_dir(path) {
        Ok(entries) => Ok(Some(entries)),
        Err(error)
            if matches!(
                error.kind(),
                io::ErrorKind::NotFound | io::ErrorKind::NotADirectory
            ) =>
        {
            Ok(None)
        }
        Err(error) => Err(error),
    }
}

fn identity_key(note: &CalendarNote) -> String {
    let (event, occurrence) = note.identity();
    format!(
        "{:x}",
        Sha256::digest(format!("{event}\n{occurrence}").as_bytes())
    )
}

/// What a note says about its event: every field but when the change was
/// noticed and whether it was first seen or changed. A removal says something
/// different from an add with the same fields.
fn content_of(note: &CalendarNote) -> String {
    let mut record = note.record.clone();
    record.remove("changed_at");
    let removed = record
        .remove("change")
        .is_some_and(|change| change == Value::String("removed".to_owned()));
    let canonical = sorted(&Value::Object(record)).to_string();
    format!(
        "{:x}",
        Sha256::digest(format!("{}{canonical}", u8::from(removed)).as_bytes())
    )
}

fn sorted(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys = map.keys().collect::<Vec<_>>();
            keys.sort();
            Value::Object(
                keys.into_iter()
                    .map(|key| (key.clone(), sorted(&map[key])))
                    .collect::<Map<_, _>>(),
            )
        }
        Value::Array(items) => Value::Array(items.iter().map(sorted).collect()),
        other => other.clone(),
    }
}

fn bucket_path(journal: &Path, name: &str) -> PathBuf {
    journal.join(RECORD_DIR).join(format!("{name}.json"))
}

fn read_bucket(journal: &Path, name: &str) -> io::Result<Bucket> {
    match std::fs::read(bucket_path(journal, name)) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(Bucket::new()),
        Err(error) => Err(error),
    }
}

#[cfg(test)]
#[path = "calendar_tests.rs"]
mod tests;
