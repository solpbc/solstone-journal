// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Obsidian vault sync with caller-owned source discovery and publishing seams.

use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::contract::{SyncPreviewRequest, SyncSaveRequest};
use crate::sync_plaud::SyncClock;
use crate::sync_state::{BackendName, SyncState, SyncStateRead, read_sync_state, write_sync_state};

/// A note prepared by a caller-owned directory scanner.
#[derive(Clone, Debug, PartialEq)]
pub struct ObsidianNote {
    pub relative_path: String,
    pub filename: String,
    pub title: String,
    /// Filesystem mtime as the reference stores it.
    pub modified_at: f64,
    pub content_hash: String,
}

/// Directory discovery and vault reads are caller-owned.
pub trait ObsidianScanner {
    fn is_directory(&self, path: &Path) -> bool;
    fn notes(&self, vault: &Path) -> Result<Vec<ObsidianNote>, String>;
}

/// Ordered home-derived candidates supplied by the caller, never read from HOME.
pub trait ObsidianHomeCandidates {
    fn candidates(&self) -> &[PathBuf];
}

/// The already-owned segment/entity write operation for one note.
pub trait ObsidianWriter {
    fn import_note(&mut self, vault: &Path, note: &ObsidianNote) -> Result<u64, String>;
}

/// Named seams that preview may use. They contain no note-write authority.
pub struct ObsidianPreviewSeams<'a> {
    pub candidates: &'a dyn ObsidianHomeCandidates,
    pub scanner: &'a dyn ObsidianScanner,
    pub clock: &'a dyn SyncClock,
}

/// Named save seams, extending preview authority with note-write access.
pub struct ObsidianSaveSeams<'a> {
    pub preview: ObsidianPreviewSeams<'a>,
    pub writer: &'a mut dyn ObsidianWriter,
}

/// Obsidian request with a type-level preview/save marker.
pub struct ObsidianSyncRequest<M> {
    pub journal_root: PathBuf,
    pub source_path: Option<PathBuf>,
    pub force: bool,
    marker: PhantomData<M>,
}

impl<M> ObsidianSyncRequest<M> {
    #[must_use]
    pub fn new(journal_root: PathBuf, source_path: Option<PathBuf>, force: bool) -> Self {
        Self {
            journal_root,
            source_path,
            force,
            marker: PhantomData,
        }
    }
}

/// Obsidian sync result.
#[derive(Clone, Debug, PartialEq)]
pub struct ObsidianSyncOutcome {
    pub state: SyncState,
    pub imported: u64,
    pub errors: Vec<String>,
}

/// Named Obsidian sync failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ObsidianSyncError {
    NoVault,
    Scan(String),
    State(String),
}

impl fmt::Display for ObsidianSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NoVault => formatter.write_str("No Obsidian vault found; supply a source path"),
            Self::Scan(message) => write!(formatter, "Obsidian scan failed: {message}"),
            Self::State(message) => write!(formatter, "Obsidian state failed: {message}"),
        }
    }
}

impl std::error::Error for ObsidianSyncError {}

pub fn sync_obsidian_preview(
    request: &ObsidianSyncRequest<SyncPreviewRequest>,
    seams: &mut ObsidianPreviewSeams<'_>,
) -> Result<ObsidianSyncOutcome, ObsidianSyncError> {
    let catalogued = catalogue(request, seams.candidates, seams.scanner, seams.clock)?;
    write_sync_state(&request.journal_root, &catalogued.state).map_err(state_error)?;
    Ok(ObsidianSyncOutcome {
        state: catalogued.state,
        imported: 0,
        errors: Vec::new(),
    })
}

pub fn sync_obsidian_save(
    request: &ObsidianSyncRequest<SyncSaveRequest>,
    seams: &mut ObsidianSaveSeams<'_>,
) -> Result<ObsidianSyncOutcome, ObsidianSyncError> {
    let mut catalogued = catalogue(
        request,
        seams.preview.candidates,
        seams.preview.scanner,
        seams.preview.clock,
    )?;
    let mut imported = 0;
    let mut errors = Vec::new();
    for note in catalogued.pending {
        let result = seams.writer.import_note(&catalogued.vault, &note);
        let entry = catalogued
            .state
            .files_mut()
            .get_mut(&note.relative_path)
            .and_then(Value::as_object_mut)
            .expect("catalogued note exists");
        match result {
            Ok(segments) => {
                entry.insert("status".to_owned(), Value::String("imported".to_owned()));
                entry.insert(
                    "imported_at".to_owned(),
                    Value::String(seams.preview.clock.now()),
                );
                entry.insert("segments".to_owned(), Value::from(segments));
                let edits = entry.get("edit_count").and_then(Value::as_u64).unwrap_or(0) + 1;
                entry.insert("edit_count".to_owned(), Value::from(edits));
                imported += 1;
            }
            Err(message) => errors.push(format!("{}: {message}", note.relative_path)),
        }
    }
    write_sync_state(&request.journal_root, &catalogued.state).map_err(state_error)?;
    Ok(ObsidianSyncOutcome {
        state: catalogued.state,
        imported,
        errors,
    })
}

struct CataloguedObsidian {
    state: SyncState,
    vault: PathBuf,
    pending: Vec<ObsidianNote>,
}

fn catalogue<M>(
    request: &ObsidianSyncRequest<M>,
    candidates: &dyn ObsidianHomeCandidates,
    scanner: &dyn ObsidianScanner,
    clock: &dyn SyncClock,
) -> Result<CataloguedObsidian, ObsidianSyncError> {
    let mut state = match read_sync_state(&request.journal_root, BackendName::Obsidian) {
        SyncStateRead::Loaded(state) => state,
        SyncStateRead::Absent | SyncStateRead::Unreadable { .. } => {
            SyncState::empty(BackendName::Obsidian)
        }
    };
    if request.force {
        state
            .root_mut()
            .insert("files".to_owned(), Value::Object(Map::new()));
    }
    let vault =
        select_vault(request, &state, candidates, scanner).ok_or(ObsidianSyncError::NoVault)?;
    let notes = scanner.notes(&vault).map_err(ObsidianSyncError::Scan)?;
    let mut seen = Vec::new();
    let mut pending = Vec::new();
    for note in notes {
        seen.push(note.relative_path.clone());
        let existing = state
            .files_mut()
            .get(&note.relative_path)
            .and_then(Value::as_object);
        let unchanged = !request.force
            && existing.is_some_and(|entry| {
                entry.get("status") == Some(&Value::String("imported".to_owned()))
                    && entry.get("content_hash") == Some(&Value::String(note.content_hash.clone()))
            });
        if unchanged {
            let entry = state
                .files_mut()
                .get_mut(&note.relative_path)
                .and_then(Value::as_object_mut)
                .expect("existing imported note exists");
            entry.insert("mtime".to_owned(), Value::from(note.modified_at));
            continue;
        }
        let entry = state
            .files_mut()
            .entry(note.relative_path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let entry = entry.as_object_mut().expect("sync-state files are objects");
        entry.insert("filename".to_owned(), Value::String(note.filename.clone()));
        entry.insert("title".to_owned(), Value::String(note.title.clone()));
        entry.insert("mtime".to_owned(), Value::from(note.modified_at));
        entry.insert(
            "content_hash".to_owned(),
            Value::String(note.content_hash.clone()),
        );
        entry.insert("status".to_owned(), Value::String("available".to_owned()));
        pending.push(note);
    }
    for (relative, entry) in state.files_mut().iter_mut() {
        if !seen.iter().any(|path| path == relative)
            && let Some(object) = entry.as_object_mut()
        {
            object.insert("status".to_owned(), Value::String("removed".to_owned()));
        }
    }
    state.root_mut().insert(
        "source_path".to_owned(),
        Value::String(vault.display().to_string()),
    );
    state
        .root_mut()
        .insert("last_sync".to_owned(), Value::String(clock.now()));
    Ok(CataloguedObsidian {
        state,
        vault,
        pending,
    })
}

fn select_vault<M>(
    request: &ObsidianSyncRequest<M>,
    state: &SyncState,
    candidates: &dyn ObsidianHomeCandidates,
    scanner: &dyn ObsidianScanner,
) -> Option<PathBuf> {
    if let Some(path) = &request.source_path {
        return scanner.is_directory(path).then(|| path.clone());
    }
    if let Some(path) = state
        .root()
        .get("source_path")
        .and_then(Value::as_str)
        .map(PathBuf::from)
        .filter(|path| scanner.is_directory(path))
    {
        return Some(path);
    }
    candidates
        .candidates()
        .iter()
        .find(|path| scanner.is_directory(path))
        .cloned()
}

fn state_error(error: impl fmt::Display) -> ObsidianSyncError {
    ObsidianSyncError::State(error.to_string())
}
