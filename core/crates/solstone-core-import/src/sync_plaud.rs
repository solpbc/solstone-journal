// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Plaud catalogue and save orchestration behind caller-owned seams.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Duration, Local, TimeZone};
use serde_json::{Map, Value};
use solstone_core_journal_io::create_directory_with_mode;

use crate::contract::{SyncPreviewRequest, SyncSaveRequest};
use crate::sync_state::{BackendName, SyncState, SyncStateRead, read_sync_state, write_sync_state};

const MIN_DURATION_MS: f64 = 30_000.0;

/// Caller-owned Plaud access credential. It is never persisted or rendered.
pub trait PlaudCredential {
    fn access_token(&self) -> Option<&str>;
}

/// Remote Plaud file metadata needed for the state catalogue.
#[derive(Clone, Debug, PartialEq)]
pub struct PlaudFile {
    pub id: String,
    pub filename: String,
    pub fullname: String,
    pub filesize: u64,
    /// Epoch seconds or milliseconds, as supplied by Plaud.
    pub start_time: f64,
    /// Recording duration in milliseconds.
    pub duration: f64,
    pub is_trash: bool,
}

/// A safe, closed description of a Plaud operation failure.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PlaudFailureKind {
    Catalogue,
    Manifest,
    TemporaryUrl,
    Download,
    Pipeline,
}

impl PlaudFailureKind {
    const fn message(self) -> &'static str {
        match self {
            Self::Catalogue => "catalogue failed",
            Self::Manifest => "import matching failed",
            Self::TemporaryUrl => "failed to get download URL",
            Self::Download => "download failed",
            Self::Pipeline => "import failed",
        }
    }
}

/// Injected remote catalogue operation. Preview authority stops at this trait.
pub trait PlaudCatalogue {
    fn list_files(&mut self, token: &str) -> Result<Vec<PlaudFile>, PlaudFailureKind>;
}

/// Injected temporary-URL and streaming-download operations, available only to save.
pub trait PlaudDownload {
    fn temporary_url(&mut self, token: &str, file_id: &str) -> Result<String, PlaudFailureKind>;
    fn download(&mut self, url: &str, destination: &Path) -> Result<(), PlaudFailureKind>;
}

/// Caller-owned match against prior import metadata.
pub trait PlaudManifestLookup {
    /// Return the import timestamp for each matched remote file ID.
    fn matching_imports(
        &self,
        files: &[PlaudFile],
    ) -> Result<BTreeMap<String, String>, PlaudFailureKind>;
}

/// Sync time is caller owned.
pub trait SyncClock {
    fn now(&self) -> String;
}

/// Auto value forwarded to the already-ported import pipeline.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PipelineAuto<'a> {
    Enabled,
    Disabled,
    Value(&'a str),
}

/// One already-approved pipeline import operation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PipelineImportRequest<'a> {
    pub source: &'a Path,
    pub source_kind: &'static str,
    pub timestamp: Option<&'a str>,
    pub auto: PipelineAuto<'a>,
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
    fn import_one(&mut self, request: PipelineImportRequest<'_>)
    -> Result<PipelineOutcome, String>;
}

/// Durable-state publication seam for Plaud catalogue and progress checkpoints.
pub trait PlaudStateWriter {
    fn checkpoint(&mut self, journal_root: &Path, state: &SyncState) -> Result<(), String>;
}

/// Production state publication through the private atomic state writer.
pub struct FilesystemPlaudStateWriter;

impl PlaudStateWriter for FilesystemPlaudStateWriter {
    fn checkpoint(&mut self, journal_root: &Path, state: &SyncState) -> Result<(), String> {
        write_sync_state(journal_root, state).map_err(|error| error.to_string())
    }
}

/// Named seams that preview may use. They contain no download or import authority.
pub struct PlaudPreviewSeams<'a> {
    pub credential: &'a dyn PlaudCredential,
    pub catalogue: &'a mut dyn PlaudCatalogue,
    pub manifests: &'a dyn PlaudManifestLookup,
    pub clock: &'a dyn SyncClock,
    pub state_writer: &'a mut dyn PlaudStateWriter,
}

/// Named save seams, extending preview authority with download and pipeline access.
pub struct PlaudSaveSeams<'a> {
    pub preview: PlaudPreviewSeams<'a>,
    pub download: &'a mut dyn PlaudDownload,
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

struct ResolvedImportTimestamp {
    timestamp: String,
    used_fallback: bool,
}

/// Named Plaud sync failure with no transport detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaudSyncError {
    MissingCredential,
    Operation(PlaudFailureKind),
    State { message: String },
}

impl fmt::Display for PlaudSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingCredential => formatter.write_str("Plaud credential is not configured"),
            Self::Operation(kind) => write!(formatter, "Plaud {}", kind.message()),
            Self::State { message } => write!(formatter, "Plaud sync state failed: {message}"),
        }
    }
}

impl std::error::Error for PlaudSyncError {}

/// Catalogue without downloading or importing.
pub fn sync_plaud_preview(
    request: &PlaudSyncRequest<SyncPreviewRequest>,
    seams: &mut PlaudPreviewSeams<'_>,
) -> Result<PlaudSyncOutcome, PlaudSyncError> {
    let catalogued = catalogue(request, seams)?;
    seams
        .state_writer
        .checkpoint(&request.journal_root, &catalogued.state)
        .map_err(state_error)?;
    Ok(PlaudSyncOutcome {
        state: catalogued.state,
        downloaded: 0,
        errors: Vec::new(),
    })
}

/// Catalogue then perform each available file's one-shot save operation.
pub fn sync_plaud_save(
    request: &PlaudSyncRequest<SyncSaveRequest>,
    seams: &mut PlaudSaveSeams<'_>,
) -> Result<PlaudSyncOutcome, PlaudSyncError> {
    let mut catalogued = catalogue(request, &mut seams.preview)?;
    let token = seams
        .preview
        .credential
        .access_token()
        .ok_or(PlaudSyncError::MissingCredential)?;
    // `sort_by` is stable, preserving catalogue order for equal start times.
    catalogued
        .available
        .sort_by(|left, right| right.start_time.total_cmp(&left.start_time));

    let mut downloaded = 0;
    let mut errors = Vec::new();
    let mut used_fallback_timestamps = HashSet::new();
    for file in catalogued.available {
        let result = import_file(
            &file,
            token,
            &request.journal_root,
            seams,
            &mut used_fallback_timestamps,
        );
        let entry = catalogued
            .state
            .files_mut()
            .get_mut(&file.id)
            .and_then(Value::as_object_mut)
            .expect("catalogued file exists");
        match result {
            Ok(resolved) => {
                entry.insert("status".to_owned(), Value::String("imported".to_owned()));
                entry.insert(
                    "import_timestamp".to_owned(),
                    Value::String(resolved.timestamp),
                );
                if resolved.used_fallback {
                    entry.insert("import_timestamp_fallback".to_owned(), Value::Bool(true));
                }
                entry.insert(
                    "imported_at".to_owned(),
                    Value::String(seams.preview.clock.now()),
                );
                entry.remove("last_error");
                downloaded += 1;
                seams
                    .preview
                    .state_writer
                    .checkpoint(&request.journal_root, &catalogued.state)
                    .map_err(state_error)?;
            }
            Err(kind) => {
                entry.insert("status".to_owned(), Value::String("available".to_owned()));
                entry.insert(
                    "last_error".to_owned(),
                    Value::String(kind.message().to_owned()),
                );
                errors.push(format!("{}: {}", file.filename, kind.message()));
            }
        }
    }
    seams
        .preview
        .state_writer
        .checkpoint(&request.journal_root, &catalogued.state)
        .map_err(state_error)?;
    Ok(PlaudSyncOutcome {
        state: catalogued.state,
        downloaded,
        errors,
    })
}

struct CataloguedPlaud {
    state: SyncState,
    available: Vec<PlaudFile>,
}

fn catalogue<M>(
    request: &PlaudSyncRequest<M>,
    seams: &mut PlaudPreviewSeams<'_>,
) -> Result<CataloguedPlaud, PlaudSyncError> {
    let token = seams
        .credential
        .access_token()
        .ok_or(PlaudSyncError::MissingCredential)?;
    let remote = seams
        .catalogue
        .list_files(token)
        .map_err(PlaudSyncError::Operation)?;
    let mut state = match read_sync_state(&request.journal_root, BackendName::Plaud) {
        SyncStateRead::Loaded(state) => state,
        SyncStateRead::Absent | SyncStateRead::Unreadable { .. } => {
            SyncState::empty(BackendName::Plaud)
        }
    };
    let needs_matching = remote
        .iter()
        .filter(|file| {
            state
                .root()
                .get("files")
                .and_then(Value::as_object)
                .and_then(|files| files.get(&file.id))
                .and_then(Value::as_object)
                .is_none_or(|entry| {
                    entry.get("status") == Some(&Value::String("available".to_owned()))
                })
        })
        .cloned()
        .collect::<Vec<_>>();
    let matches = seams
        .manifests
        .matching_imports(&needs_matching)
        .map_err(PlaudSyncError::Operation)?;

    let mut available = Vec::new();
    for file in remote {
        let entry = state
            .files_mut()
            .entry(file.id.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let object = entry.as_object_mut().expect("sync-state files are objects");
        let existed = !object.is_empty();
        object.insert("filename".to_owned(), Value::String(file.filename.clone()));
        object.insert("filesize".to_owned(), Value::from(file.filesize));
        if existed {
            if object.get("status") == Some(&Value::String("available".to_owned()))
                && let Some(timestamp) = matches.get(&file.id)
            {
                object.insert("status".to_owned(), Value::String("imported".to_owned()));
                object.insert(
                    "import_timestamp".to_owned(),
                    Value::String(timestamp.clone()),
                );
                object.insert("matched_at".to_owned(), Value::String(seams.clock.now()));
                object.remove("last_error");
            }
            if object.get("status") == Some(&Value::String("available".to_owned())) {
                available.push(file);
            }
            continue;
        }

        object.insert("fullname".to_owned(), Value::String(file.fullname.clone()));
        object.insert("start_time".to_owned(), Value::from(file.start_time));
        object.insert("duration".to_owned(), Value::from(file.duration));
        object.insert("is_trash".to_owned(), Value::Bool(file.is_trash));
        if let Some(timestamp) = matches.get(&file.id) {
            object.insert("status".to_owned(), Value::String("imported".to_owned()));
            object.insert(
                "import_timestamp".to_owned(),
                Value::String(timestamp.clone()),
            );
            object.insert("matched_at".to_owned(), Value::String(seams.clock.now()));
        } else if file.is_trash {
            object.insert("status".to_owned(), Value::String("skipped".to_owned()));
            object.insert(
                "skip_reason".to_owned(),
                Value::String("trashed".to_owned()),
            );
        } else if file.duration > 0.0 && file.duration < MIN_DURATION_MS {
            object.insert("status".to_owned(), Value::String("skipped".to_owned()));
            object.insert(
                "skip_reason".to_owned(),
                Value::String("too_short".to_owned()),
            );
        } else {
            object.insert("status".to_owned(), Value::String("available".to_owned()));
            available.push(file);
        }
    }
    state
        .root_mut()
        .insert("last_sync".to_owned(), Value::String(seams.clock.now()));
    Ok(CataloguedPlaud { state, available })
}

fn import_file(
    file: &PlaudFile,
    token: &str,
    journal_root: &Path,
    seams: &mut PlaudSaveSeams<'_>,
    used_fallback_timestamps: &mut HashSet<String>,
) -> Result<ResolvedImportTimestamp, PlaudFailureKind> {
    let now = seams.preview.clock.now();
    let resolved = import_timestamp(file.start_time, &now, used_fallback_timestamps)
        .ok_or(PlaudFailureKind::Pipeline)?;
    let destination_dir = journal_root.join("imports").join(&resolved.timestamp);
    create_directory_with_mode(&destination_dir, 0o700).map_err(|_| PlaudFailureKind::Download)?;
    let url = seams.download.temporary_url(token, &file.id)?;
    let destination = destination_dir.join(destination_name(file));
    seams.download.download(&url, &destination)?;
    match seams.pipeline.import_one(PipelineImportRequest {
        source: &destination,
        source_kind: "plaud",
        timestamp: Some(&resolved.timestamp),
        auto: PipelineAuto::Enabled,
    }) {
        Ok(PipelineOutcome::Imported) => Ok(resolved),
        Ok(PipelineOutcome::Skipped { .. })
        | Ok(PipelineOutcome::NoResult)
        | Ok(PipelineOutcome::Unrecognized)
        | Err(_) => Err(PlaudFailureKind::Pipeline),
    }
}

fn import_timestamp(
    start_time: f64,
    now: &str,
    used_fallback_timestamps: &mut HashSet<String>,
) -> Option<ResolvedImportTimestamp> {
    if !start_time.is_finite() || start_time <= 0.0 {
        return None;
    }
    let seconds = if start_time > 1_000_000_000_000.0 {
        start_time / 1000.0
    } else {
        start_time
    };
    if seconds > i64::MAX as f64 {
        return None;
    }
    let device_time = Local.timestamp_opt(seconds as i64, 0).single()?;
    let now = DateTime::parse_from_rfc3339(now).ok()?;
    if device_time.signed_duration_since(now) <= Duration::hours(48) {
        return Some(ResolvedImportTimestamp {
            timestamp: device_time.format("%Y%m%d_%H%M%S").to_string(),
            used_fallback: false,
        });
    }

    let mut fallback_time = now;
    loop {
        let timestamp = fallback_time.format("%Y%m%d_%H%M%S").to_string();
        if used_fallback_timestamps.insert(timestamp.clone()) {
            return Some(ResolvedImportTimestamp {
                timestamp,
                used_fallback: true,
            });
        }
        fallback_time = fallback_time.checked_add_signed(Duration::seconds(1))?;
    }
}

fn destination_name(file: &PlaudFile) -> String {
    let extension = Path::new(&file.fullname)
        .extension()
        .filter(|extension| !extension.is_empty())
        .map(|extension| format!(".{}", extension.to_string_lossy()))
        .unwrap_or_else(|| ".opus".to_owned());
    format!("{}{}", sanitize_filename(&file.filename), extension)
}

fn sanitize_filename(filename: &str) -> String {
    let mut rendered = String::new();
    let mut separator = false;
    for character in filename.chars() {
        let replacement = matches!(
            character,
            '<' | '>' | ':' | '"' | '/' | '\\' | '|' | '?' | '*'
        );
        if replacement || character.is_whitespace() || character == '_' {
            if !separator {
                rendered.push('_');
                separator = true;
            }
        } else {
            rendered.push(character);
            separator = false;
        }
    }
    let trimmed = rendered.trim_matches('_');
    if trimmed.is_empty() {
        "unnamed".to_owned()
    } else {
        trimmed.to_owned()
    }
}

fn state_error(message: String) -> PlaudSyncError {
    PlaudSyncError::State { message }
}
