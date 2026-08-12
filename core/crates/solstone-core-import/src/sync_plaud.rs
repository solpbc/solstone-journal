// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Plaud catalogue and save orchestration behind caller-owned seams.

use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};
use solstone_core_journal_io::create_directory_with_mode;

use crate::contract::{SyncPreviewRequest, SyncSaveRequest};
use crate::sync_state::{BackendName, SyncState, SyncStateRead, read_sync_state, write_sync_state};

/// Caller-owned Plaud access credential. It is never persisted or rendered.
pub trait PlaudCredential {
    fn access_token(&self) -> Option<&str>;
}

/// Remote Plaud file metadata needed for the state catalogue.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaudFile {
    pub id: String,
    pub filename: String,
    pub filesize: u64,
    pub start_time: String,
    pub trashed: bool,
}

/// A caller-owned HTTP failure with safe, credential-free detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PlaudHttpError {
    pub message: String,
}

/// Injected catalogue, temporary-URL, and streaming-download transport.
pub trait PlaudHttp {
    fn list_files(&mut self, token: &str) -> Result<Vec<PlaudFile>, PlaudHttpError>;
    fn temporary_url(&mut self, token: &str, file_id: &str) -> Result<String, PlaudHttpError>;
    fn download(&mut self, url: &str, destination: &Path) -> Result<(), PlaudHttpError>;
}

/// Sync time is caller owned.
pub trait SyncClock {
    fn now(&self) -> String;
}

/// Result returned by the already-ported import pipeline seam.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PipelineOutcome {
    Imported,
    Skipped { reason: String },
    NoResult,
    Unrecognized,
}

/// The import pipeline standing in for Python `import_one`.
pub trait ImportPipeline {
    fn import_one(
        &mut self,
        source: &Path,
        source_kind: &str,
        auto: bool,
    ) -> Result<PipelineOutcome, String>;
}

/// Named caller-owned seams for Plaud sync.
pub struct PlaudSyncSeams<'a> {
    pub credential: &'a dyn PlaudCredential,
    pub http: &'a mut dyn PlaudHttp,
    pub clock: &'a dyn SyncClock,
    pub pipeline: &'a mut dyn ImportPipeline,
}

/// A Plaud request whose mode is fixed at the type boundary.
pub struct PlaudSyncRequest<M> {
    pub journal_root: PathBuf,
    marker: PhantomData<M>,
}

impl<M> PlaudSyncRequest<M> {
    #[must_use]
    pub fn new(journal_root: PathBuf) -> Self {
        Self {
            journal_root,
            marker: PhantomData,
        }
    }
}

/// Summary returned by Plaud sync.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaudSyncOutcome {
    pub state: SyncState,
    pub downloaded: u64,
    pub errors: Vec<String>,
}

/// Named Plaud sync failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaudSyncError {
    MissingCredential,
    Http {
        operation: &'static str,
        message: String,
    },
    State {
        message: String,
    },
}

impl fmt::Display for PlaudSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCredential => formatter.write_str("Plaud credential is not configured"),
            Self::Http { operation, message } => {
                write!(formatter, "Plaud {operation} failed: {message}")
            }
            Self::State { message } => write!(formatter, "Plaud sync state failed: {message}"),
        }
    }
}

impl std::error::Error for PlaudSyncError {}

/// Catalogue without downloading or importing.
pub fn sync_plaud_preview(
    request: &PlaudSyncRequest<SyncPreviewRequest>,
    seams: &mut PlaudSyncSeams<'_>,
) -> Result<PlaudSyncOutcome, PlaudSyncError> {
    catalog(request, seams, false)
}

/// Catalogue then perform each available file's one-shot save operation.
pub fn sync_plaud_save(
    request: &PlaudSyncRequest<SyncSaveRequest>,
    seams: &mut PlaudSyncSeams<'_>,
) -> Result<PlaudSyncOutcome, PlaudSyncError> {
    catalog(request, seams, true)
}

fn catalog<M>(
    request: &PlaudSyncRequest<M>,
    seams: &mut PlaudSyncSeams<'_>,
    save: bool,
) -> Result<PlaudSyncOutcome, PlaudSyncError> {
    let token = seams
        .credential
        .access_token()
        .ok_or(PlaudSyncError::MissingCredential)?;
    let remote = seams
        .http
        .list_files(token)
        .map_err(|error| http_error("catalogue", error))?;
    let mut state = match read_sync_state(&request.journal_root, BackendName::Plaud) {
        SyncStateRead::Loaded(state) => state,
        SyncStateRead::Absent | SyncStateRead::Unreadable { .. } => {
            SyncState::empty(BackendName::Plaud)
        }
    };
    let mut available = Vec::new();
    for file in remote {
        let entry = state
            .files_mut()
            .entry(file.id.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let object = entry.as_object_mut().expect("sync-state files are objects");
        object.insert("filename".to_owned(), Value::String(file.filename.clone()));
        object.insert("filesize".to_owned(), Value::from(file.filesize));
        object.insert(
            "start_time".to_owned(),
            Value::String(file.start_time.clone()),
        );
        if file.trashed {
            object.insert("status".to_owned(), Value::String("skipped".to_owned()));
            object.insert(
                "skip_reason".to_owned(),
                Value::String("trashed".to_owned()),
            );
        } else if object.get("status") != Some(&Value::String("imported".to_owned())) {
            object.insert("status".to_owned(), Value::String("available".to_owned()));
            available.push(file);
        }
    }
    state
        .root_mut()
        .insert("last_sync".to_owned(), Value::String(seams.clock.now()));
    if !save {
        write_sync_state(&request.journal_root, &state).map_err(state_error)?;
        return Ok(PlaudSyncOutcome {
            state,
            downloaded: 0,
            errors: Vec::new(),
        });
    }

    let destination_dir = request.journal_root.join("imports").join("plaud");
    create_directory_with_mode(&destination_dir, 0o700).map_err(|error| PlaudSyncError::State {
        message: error.to_string(),
    })?;
    let mut downloaded = 0;
    let mut errors = Vec::new();
    for file in available {
        let result = import_file(&file, token, &destination_dir, seams);
        let entry = state
            .files_mut()
            .get_mut(&file.id)
            .and_then(Value::as_object_mut)
            .expect("catalogued file exists");
        match result {
            Ok(()) => {
                entry.insert("status".to_owned(), Value::String("imported".to_owned()));
                entry.insert("imported_at".to_owned(), Value::String(seams.clock.now()));
                entry.remove("last_error");
                downloaded += 1;
                write_sync_state(&request.journal_root, &state).map_err(state_error)?;
            }
            Err(message) => {
                entry.insert("status".to_owned(), Value::String("available".to_owned()));
                entry.insert("last_error".to_owned(), Value::String(message.clone()));
                errors.push(format!("{}: {message}", file.filename));
            }
        }
    }
    write_sync_state(&request.journal_root, &state).map_err(state_error)?;
    Ok(PlaudSyncOutcome {
        state,
        downloaded,
        errors,
    })
}

fn import_file(
    file: &PlaudFile,
    token: &str,
    destination_dir: &Path,
    seams: &mut PlaudSyncSeams<'_>,
) -> Result<(), String> {
    let url = seams
        .http
        .temporary_url(token, &file.id)
        .map_err(|error| error.message)?;
    let destination = destination_dir.join(&file.id);
    seams
        .http
        .download(&url, &destination)
        .map_err(|error| error.message)?;
    match seams
        .pipeline
        .import_one(&destination, "plaud", true)
        .map_err(|error| error.to_string())?
    {
        PipelineOutcome::Imported => Ok(()),
        PipelineOutcome::Skipped { reason } => Err(format!("import skipped: {reason}")),
        PipelineOutcome::NoResult => Err("import returned no result".to_owned()),
        PipelineOutcome::Unrecognized => Err("import returned unrecognized result".to_owned()),
    }
}

fn http_error(operation: &'static str, error: PlaudHttpError) -> PlaudSyncError {
    PlaudSyncError::Http {
        operation,
        message: error.message,
    }
}

fn state_error(error: impl fmt::Display) -> PlaudSyncError {
    PlaudSyncError::State {
        message: error.to_string(),
    }
}
