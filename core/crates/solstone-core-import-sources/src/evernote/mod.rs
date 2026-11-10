// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! An Evernote export brought in as notes.
//!
//! The private record of what already came in lives under
//! `config/evernote-imports/`. The path is a hash. The notebook name the owner
//! confirmed is stored inside the file.

mod parse;

pub mod copy;
pub use parse::{
    HeldBytes, MAX_EXPORT_BYTES, MAX_RESOURCE_BYTES, ParsedNote, ReadExport, looks_like_export,
    proposed_notebook, read_export,
};

use std::collections::HashMap;
use std::fs::{self, File};
use std::io::{BufReader, Read};
use std::path::{Path, PathBuf};

use chrono::{DateTime, NaiveDate, TimeZone, Timelike, Utc};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_format::obsidian::{
    NOTE_TRANSCRIPT_FILE, NoteEntry, render_edit, render_transcript,
};
use solstone_core_journal_io::{AtomicWriteOptions, atomic_replace};

use crate::shared::{self, WINDOW_SECONDS};

const STREAM: &str = "import.evernote";
const PRIVATE_FILE_MODE: u32 = 0o600;
const PRIVATE_DIR_MODE: u32 = 0o700;

/// Whether these bytes are past the export ceiling.
#[must_use]
pub fn exceeds_export_ceiling(bytes: u64) -> bool {
    bytes > MAX_EXPORT_BYTES
}

/// Whether this file claims to be an Evernote export.
#[must_use]
pub fn claims_path(path: &Path) -> bool {
    let Ok(mut file) = File::open(path) else {
        return false;
    };
    let mut head = [0_u8; 8192];
    let Ok(read) = file.read(&mut head) else {
        return false;
    };
    looks_like_export(&head[..read])
}

/// What the page shows before anything is written.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct FilePreview {
    pub notebook: String,
    pub lines: Vec<String>,
    pub ready: bool,
    pub refusal: Option<String>,
}

/// Read an export for the preview. Nothing is written.
pub fn preview_path(path: &Path) -> Result<FilePreview, String> {
    let notebook = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(proposed_notebook)
        .unwrap_or_default();
    let export = open_export(path)?;
    Ok(preview_export(&export, &notebook))
}

fn preview_export(export: &ReadExport, notebook: &str) -> FilePreview {
    let ReadExport::Export(parsed) = export else {
        return FilePreview {
            notebook: notebook.to_owned(),
            lines: vec![copy::NOT_AN_EXPORT.to_owned()],
            ready: false,
            refusal: Some(copy::NOT_AN_EXPORT.to_owned()),
        };
    };
    if parsed.notes.is_empty() {
        let line = if parsed.ended_early {
            copy::ended_early(0)
        } else {
            copy::EMPTY_EXPORT.to_owned()
        };
        return FilePreview {
            notebook: notebook.to_owned(),
            lines: vec![line.clone()],
            ready: false,
            refusal: Some(line),
        };
    }
    let mut years = Vec::new();
    let mut attachments = 0_usize;
    let mut encrypted = 0_usize;
    for note in &parsed.notes {
        attachments += note.attachment_count;
        if note.encrypted {
            encrypted += 1;
        }
        if let Some(time) = parse::memory_time(note) {
            years.push(time.format("%Y").to_string().parse::<i32>().unwrap_or(0));
        }
    }
    let mut lines = Vec::new();
    if let (Some(first), Some(last)) = (years.iter().min(), years.iter().max()) {
        lines.push(copy::preview_counts(
            parsed.notes.len(),
            *first,
            *last,
            attachments,
        ));
    } else {
        lines.push(copy::preview_counts_undated(
            parsed.notes.len(),
            attachments,
        ));
    }
    if encrypted > 0 {
        lines.push(copy::encrypted_count(encrypted));
    }
    lines.push(copy::NOTEBOOK_HINT.to_owned());
    FilePreview {
        notebook: notebook.to_owned(),
        lines,
        ready: true,
        refusal: None,
    }
}

/// One note the private record remembers.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct StoredNote {
    pub notebook: String,
    pub identity_second: String,
    pub disambiguator: Option<String>,
    pub original_title: String,
    pub last_title: String,
    pub content_hash: String,
    pub rendered: String,
    pub time: String,
    pub day: String,
    pub segment: String,
}

/// A note file to write into `import.evernote`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedFile {
    pub day: String,
    pub segment: String,
    pub contents: String,
    pub units: u64,
}

/// Bytes that go through the image or document importer with the note's time.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlannedBinary {
    pub file_name: String,
    pub bytes: Vec<u8>,
    pub owner_timestamp: String,
}

/// What this export would change. Nothing is written until [`commit`].
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Plan {
    pub refusal: Option<String>,
    pub summary: String,
    pub files: Vec<PlannedFile>,
    pub images: Vec<PlannedBinary>,
    pub pdfs: Vec<PlannedBinary>,
    pub entries: u64,
    pub updates: Vec<StoredNote>,
    pub notebook: String,
}

/// Plan an export against the notes already stored for `notebook`.
pub fn plan<Z>(
    journal: &Path,
    export_path: &Path,
    notebook: &str,
    zone: &Z,
    today: NaiveDate,
) -> Result<Plan, String>
where
    Z: TimeZone,
    Z::Offset: std::fmt::Display,
{
    let notebook = notebook.trim();
    if notebook.is_empty() {
        return Err("the notebook name is empty".to_owned());
    }
    let export = open_export(export_path)?;
    let ReadExport::Export(parsed) = export else {
        return Ok(refused(notebook, copy::NOT_AN_EXPORT));
    };
    if parsed.notes.is_empty() && !parsed.ended_early {
        return Ok(refused(notebook, copy::EMPTY_EXPORT));
    }
    let stored = load_state(journal, notebook)?;
    Ok(decide(
        journal,
        notebook,
        &parsed.notes,
        &stored,
        parsed.ended_early,
        zone,
        today,
    )?)
}

/// Write the private record after the segments exist.
pub fn commit(journal: &Path, notes: &[StoredNote]) -> Result<(), String> {
    for note in notes {
        let path = note_path(journal, note);
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|error| error.to_string())?;
            tighten(parent, PRIVATE_DIR_MODE)?;
            if let Some(notebook_dir) = parent.parent() {
                tighten(notebook_dir, PRIVATE_DIR_MODE)?;
            }
        }
        let bytes = serde_json::to_vec_pretty(note).map_err(|error| error.to_string())?;
        atomic_replace(
            &path,
            &bytes,
            AtomicWriteOptions {
                mode: Some(PRIVATE_FILE_MODE),
            },
        )
        .map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn refused(notebook: &str, line: &str) -> Plan {
    Plan {
        refusal: Some(line.to_owned()),
        summary: line.to_owned(),
        files: Vec::new(),
        images: Vec::new(),
        pdfs: Vec::new(),
        entries: 0,
        updates: Vec::new(),
        notebook: notebook.to_owned(),
    }
}

fn open_export(path: &Path) -> Result<ReadExport, String> {
    let file = File::open(path).map_err(|error| error.to_string())?;
    Ok(read_export(BufReader::new(file)))
}

enum Step {
    Undated,
    Future,
    Older,
    Unchanged {
        index: usize,
        time: Option<DateTime<Utc>>,
    },
    Trash,
    Edit {
        index: usize,
        body: String,
        memory: DateTime<Utc>,
        title: String,
    },
    New {
        index: usize,
        memory: DateTime<Utc>,
        title: String,
        body: String,
        images: Vec<HeldBytes>,
        pdfs: Vec<HeldBytes>,
    },
}

fn decide<Z>(
    journal: &Path,
    notebook: &str,
    notes: &[ParsedNote],
    stored: &[StoredNote],
    ended_early: bool,
    zone: &Z,
    today: NaiveDate,
) -> Result<Plan, String>
where
    Z: TimeZone,
    Z::Offset: std::fmt::Display,
{
    let mut working = stored.to_vec();
    let mut steps = Vec::with_capacity(notes.len());
    let mut undated = 0_u64;
    let mut future = 0_u64;
    let mut trash = false;
    let mut matched = 0_u64;

    for note in notes {
        let Some(identity) = parse::identity_time(note) else {
            undated += 1;
            steps.push(Step::Undated);
            continue;
        };
        let Some(memory) = parse::memory_time(note) else {
            undated += 1;
            steps.push(Step::Undated);
            continue;
        };
        let local_day = memory.with_timezone(zone).date_naive();
        if local_day > today {
            future += 1;
            steps.push(Step::Future);
            continue;
        }
        let second = stamp(&identity);
        let title = heading_title(&note.title);
        let rendered = normalize(&note.rendered);
        let candidates = indexes_for(&working, &second);
        let content_hits: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&index| working[index].content_hash == note.content_hash)
            .collect();
        let title_hits: Vec<usize> = candidates
            .iter()
            .copied()
            .filter(|&index| title_matches(&working[index], &title))
            .collect();
        let matched_index = if content_hits.len() == 1 {
            Some(content_hits[0])
        } else if content_hits.is_empty() && title_hits.len() == 1 {
            Some(title_hits[0])
        } else {
            None
        };
        if let Some(index) = matched_index {
            matched += 1;
            if !segment_live(journal, &working[index].day, &working[index].segment)? {
                trash = true;
                steps.push(Step::Trash);
                continue;
            }
            let stored_time = parse_stamp(&working[index].time).unwrap_or(memory);
            let differs = normalize(&working[index].rendered) != rendered;
            if !differs {
                if memory >= stored_time && working[index].last_title != title {
                    working[index].last_title = title.clone();
                }
                let time = (memory > stored_time).then_some(memory);
                steps.push(Step::Unchanged { index, time });
                continue;
            }
            if memory < stored_time {
                steps.push(Step::Older);
                continue;
            }
            let Some(body) = render_edit(&working[index].rendered, &rendered) else {
                let time = (memory > stored_time).then_some(memory);
                steps.push(Step::Unchanged { index, time });
                continue;
            };
            steps.push(Step::Edit {
                index,
                body,
                memory,
                title,
            });
        } else {
            let disambiguator = if candidates.is_empty() {
                None
            } else if !title_taken(&working, &candidates, &title) {
                Some(title.clone())
            } else {
                Some(note.content_hash.clone())
            };
            let index = working.len();
            working.push(StoredNote {
                notebook: notebook.to_owned(),
                identity_second: second,
                disambiguator,
                original_title: title.clone(),
                last_title: title.clone(),
                content_hash: note.content_hash.clone(),
                rendered: rendered.clone(),
                time: stamp(&memory),
                day: String::new(),
                segment: String::new(),
            });
            steps.push(Step::New {
                index,
                memory,
                title,
                body: rendered,
                images: note.images.clone(),
                pdfs: note.pdfs.clone(),
            });
        }
    }

    // New notes and edits that leave their old window share one placement, so
    // notes in the same few minutes land in one transcript, including a second
    // window that opens on the same wall-clock second.
    let mut placeable = Vec::new();
    for (step_index, step) in steps.iter().enumerate() {
        match step {
            Step::New { memory, .. } => placeable.push((*memory, step_index)),
            Step::Edit { index, memory, .. }
                if !same_window(
                    &working[*index].day,
                    &working[*index].segment,
                    *memory,
                    zone,
                ) =>
            {
                placeable.push((*memory, step_index));
            }
            _ => {}
        }
    }
    for window in shared::window(placeable, zone) {
        let day = window.day.clone();
        let segment = window.segment_key.clone();
        for (_time, step_index) in window.items {
            let index = match &steps[step_index] {
                Step::New { index, .. } | Step::Edit { index, .. } => *index,
                _ => continue,
            };
            working[index].day.clone_from(&day);
            working[index].segment.clone_from(&segment);
        }
    }

    let mut writes: Vec<Write> = Vec::new();
    let mut images = Vec::new();
    let mut pdfs = Vec::new();
    let mut new_count = 0_u64;
    let mut changed = 0_u64;
    for step in &steps {
        match step {
            Step::Unchanged {
                index,
                time: Some(time),
            } => {
                working[*index].time = stamp(time);
            }
            Step::Edit {
                index,
                body,
                memory,
                title,
            } => {
                let day = working[*index].day.clone();
                let segment = working[*index].segment.clone();
                working[*index].last_title = title.clone();
                working[*index].rendered = body.clone();
                working[*index].time = stamp(memory);
                writes.push(Write {
                    day,
                    segment,
                    entry: NoteEntry {
                        path: format!("{title}.md"),
                        status: Some("edited".to_owned()),
                        body: body.clone(),
                    },
                });
                changed += 1;
            }
            Step::New {
                index,
                memory,
                title,
                body,
                images: note_images,
                pdfs: note_pdfs,
            } => {
                let day = working[*index].day.clone();
                let segment = working[*index].segment.clone();
                let owner_timestamp = memory
                    .with_timezone(zone)
                    .format("%Y%m%d_%H%M%S")
                    .to_string();
                for image in note_images {
                    images.push(PlannedBinary {
                        file_name: image.file_name.clone(),
                        bytes: image.bytes.clone(),
                        owner_timestamp: owner_timestamp.clone(),
                    });
                }
                for pdf in note_pdfs {
                    pdfs.push(PlannedBinary {
                        file_name: pdf.file_name.clone(),
                        bytes: pdf.bytes.clone(),
                        owner_timestamp: owner_timestamp.clone(),
                    });
                }
                writes.push(Write {
                    day,
                    segment,
                    entry: NoteEntry {
                        path: format!("{title}.md"),
                        status: None,
                        body: body.clone(),
                    },
                });
                new_count += 1;
            }
            Step::Undated
            | Step::Future
            | Step::Older
            | Step::Trash
            | Step::Unchanged { time: None, .. } => {}
        }
    }

    // An edit stores the edit body, not the full note. Keep the full rendered
    // text on the private record so the next comparison sees the note.
    for (note, original) in notes.iter().zip(steps.iter()) {
        if let Step::Edit { index, .. } = original {
            working[*index].rendered = normalize(&note.rendered);
            working[*index].content_hash = note.content_hash.clone();
        }
    }

    let files = compose(journal, &writes)?;
    let summary = finish_lines(
        new_count,
        changed,
        (images.len() + pdfs.len()) as u64,
        future,
        undated,
        ended_early,
        trash,
        matched,
    );
    let updates = changed_notes(stored, &working);
    Ok(Plan {
        refusal: None,
        summary,
        files,
        images,
        pdfs,
        entries: new_count + changed,
        updates,
        notebook: notebook.to_owned(),
    })
}

struct Write {
    day: String,
    segment: String,
    entry: NoteEntry,
}

fn compose(journal: &Path, writes: &[Write]) -> Result<Vec<PlannedFile>, String> {
    let mut order = Vec::new();
    let mut groups: HashMap<(String, String), Vec<NoteEntry>> = HashMap::new();
    for write in writes {
        let key = (write.day.clone(), write.segment.clone());
        if !groups.contains_key(&key) {
            order.push(key.clone());
        }
        groups.entry(key).or_default().push(write.entry.clone());
    }
    let mut files = Vec::new();
    for key in order {
        let entries = groups.remove(&key).unwrap_or_default();
        if !segment_live(journal, &key.0, &key.1)? && segment_dir(journal, &key.0, &key.1).is_dir()
        {
            continue;
        }
        let prior = transcript_text(journal, &key.0, &key.1);
        let contents = match prior {
            Some(mut body) if !body.is_empty() => {
                for entry in &entries {
                    if !body.ends_with('\n') {
                        body.push('\n');
                    }
                    body.push('\n');
                    body.push_str(&render_transcript(std::slice::from_ref(entry)));
                }
                body
            }
            _ => render_transcript(&entries),
        };
        files.push(PlannedFile {
            day: key.0,
            segment: key.1,
            contents,
            units: u64::try_from(entries.len()).unwrap_or(0),
        });
    }
    Ok(files)
}

fn finish_lines(
    new_count: u64,
    changed: u64,
    attachments: u64,
    future: u64,
    undated: u64,
    ended_early: bool,
    trash: bool,
    matched: u64,
) -> String {
    let mut lines = Vec::new();
    if new_count > 0 && changed > 0 {
        lines.push(copy::brought_in_new_and_changed(new_count, changed));
    } else if new_count > 0 {
        lines.push(copy::brought_in(new_count));
    } else if changed > 0 {
        lines.push(copy::brought_in_changed(changed));
    }
    if attachments > 0 {
        lines.push(copy::attachments_came_in(attachments));
    }
    if future > 0 {
        lines.push(copy::dated_after_today(future));
    }
    if undated > 0 {
        lines.push(copy::no_date(undated));
    }
    if ended_early {
        lines.push(copy::ended_early(new_count));
    }
    if trash {
        lines.push(copy::IN_THE_TRASH.to_owned());
    }
    if new_count == 0 && changed == 0 && matched > 0 && !trash {
        lines.push(copy::ALREADY_IN_JOURNAL.to_owned());
    }
    lines.join("\n")
}

fn indexes_for(notes: &[StoredNote], second: &str) -> Vec<usize> {
    notes
        .iter()
        .enumerate()
        .filter(|(_, note)| note.identity_second == second)
        .map(|(index, _)| index)
        .collect()
}

fn title_matches(note: &StoredNote, title: &str) -> bool {
    note.disambiguator.as_deref() == Some(title)
        || note.original_title == title
        || note.last_title == title
}

fn title_taken(notes: &[StoredNote], candidates: &[usize], title: &str) -> bool {
    candidates
        .iter()
        .any(|&index| title_matches(&notes[index], title))
}

fn same_window<Z>(day: &str, segment: &str, memory: DateTime<Utc>, zone: &Z) -> bool
where
    Z: TimeZone,
    Z::Offset: std::fmt::Display,
{
    if day.is_empty() || shared::day_key(memory, zone) != day {
        return false;
    }
    let Some(start) = segment_start(segment) else {
        return false;
    };
    let local = memory.with_timezone(zone);
    let seconds =
        i64::from(local.hour()) * 3600 + i64::from(local.minute()) * 60 + i64::from(local.second());
    seconds >= start && seconds - start < WINDOW_SECONDS
}

fn segment_start(segment: &str) -> Option<i64> {
    let hhmmss = segment.split('_').next()?;
    if hhmmss.len() != 6 {
        return None;
    }
    let hour: i64 = hhmmss[0..2].parse().ok()?;
    let minute: i64 = hhmmss[2..4].parse().ok()?;
    let second: i64 = hhmmss[4..6].parse().ok()?;
    Some(hour * 3600 + minute * 60 + second)
}

fn segment_live(journal: &Path, day: &str, segment: &str) -> Result<bool, String> {
    if day.is_empty() || segment.is_empty() {
        return Ok(true);
    }
    let dir = segment_dir(journal, day, segment);
    if !dir.is_dir() {
        return Ok(false);
    }
    match solstone_core_segment::owner_deletion_state(&dir) {
        Ok(solstone_core_segment::OwnerDeletionState::Live) => Ok(true),
        Ok(_) => Ok(false),
        Err(error) => Err(error.to_string()),
    }
}

fn segment_dir(journal: &Path, day: &str, segment: &str) -> PathBuf {
    journal
        .join("chronicle")
        .join(day)
        .join(STREAM)
        .join(segment)
}

fn transcript_text(journal: &Path, day: &str, segment: &str) -> Option<String> {
    let path = segment_dir(journal, day, segment).join(NOTE_TRANSCRIPT_FILE);
    fs::read_to_string(path).ok()
}

fn changed_notes(before: &[StoredNote], after: &[StoredNote]) -> Vec<StoredNote> {
    let mut prior = HashMap::new();
    for note in before {
        prior.insert(note_id(note), note);
    }
    after
        .iter()
        .filter(|note| prior.get(&note_id(note)).is_none_or(|old| *old != *note))
        .cloned()
        .collect()
}

fn load_state(journal: &Path, notebook: &str) -> Result<Vec<StoredNote>, String> {
    let dir = journal
        .join("config")
        .join("evernote-imports")
        .join(notebook_id(notebook))
        .join("notes");
    if !dir.is_dir() {
        return Ok(Vec::new());
    }
    let mut notes = Vec::new();
    for entry in fs::read_dir(&dir).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
            continue;
        }
        let bytes = fs::read(&path).map_err(|error| error.to_string())?;
        let note: StoredNote = serde_json::from_slice(&bytes).map_err(|error| error.to_string())?;
        if note.notebook == notebook {
            notes.push(note);
        }
    }
    Ok(notes)
}

fn note_path(journal: &Path, note: &StoredNote) -> PathBuf {
    journal
        .join("config")
        .join("evernote-imports")
        .join(notebook_id(&note.notebook))
        .join("notes")
        .join(format!("{}.json", note_id(note)))
}

fn notebook_id(name: &str) -> String {
    hex_sha256(name.as_bytes())
}

fn note_id(note: &StoredNote) -> String {
    let disambiguator = note.disambiguator.as_deref().unwrap_or("");
    hex_sha256(format!("{}\n{disambiguator}", note.identity_second).as_bytes())
}

fn hex_sha256(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

fn stamp(time: &DateTime<Utc>) -> String {
    time.format("%Y%m%dT%H%M%SZ").to_string()
}

fn parse_stamp(raw: &str) -> Option<DateTime<Utc>> {
    let naive = chrono::NaiveDateTime::parse_from_str(raw, "%Y%m%dT%H%M%SZ").ok()?;
    Some(DateTime::<Utc>::from_naive_utc_and_offset(naive, Utc))
}

fn normalize(text: &str) -> String {
    text.replace("\r\n", "\n")
}

fn heading_title(title: &str) -> String {
    let trimmed = title.trim();
    let title = if trimmed.is_empty() {
        "Untitled"
    } else {
        trimmed
    };
    title
        .chars()
        .map(|c| match c {
            '/' | '\\' | '\n' | '\r' => ' ',
            other => other,
        })
        .collect()
}

#[cfg(test)]
mod tests;

fn tighten(path: &Path, mode: u32) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(mode))
            .map_err(|error| error.to_string())?;
    }
    #[cfg(not(unix))]
    {
        let _ = (path, mode);
    }
    Ok(())
}
