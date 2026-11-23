// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Plaud catalogue and save orchestration behind caller-owned seams.

use std::collections::{BTreeMap, HashSet};
use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use jiff::tz::TimeZone;
use jiff::{SignedDuration, Timestamp};
use serde_json::{Map, Value};

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
    /// Plaud refused the sign-in: it expired or was revoked, so the owner needs to sign in again.
    TokenRefused,
    Catalogue,
    Manifest,
    TemporaryUrl,
    /// Plaud returned `presigned_url: null` while audio finishes preparing; retry on next sync.
    AudioNotReady,
    Download,
    Pipeline,
}

impl PlaudFailureKind {
    #[must_use]
    pub fn render(self, locale: &str) -> String {
        let id = match self {
            Self::TokenRefused => "journal.cli.importer.plaud.failure.token.refused",
            Self::Catalogue => "journal.cli.importer.plaud.failure.catalogue",
            Self::Manifest => "journal.cli.importer.plaud.failure.manifest",
            Self::TemporaryUrl => "journal.cli.importer.plaud.failure.temporary.url",
            Self::AudioNotReady => "journal.cli.importer.plaud.failure.audio.not.ready",
            Self::Download => "journal.cli.importer.plaud.failure.download",
            Self::Pipeline => "journal.cli.importer.plaud.failure.pipeline",
        };
        solstone_core_cli_copy::render_for(locale, id, &[])
    }

    const fn message(self) -> &'static str {
        match self {
            Self::TokenRefused => "sign-in expired or was revoked; sign in again in settings",
            Self::Catalogue => "catalogue failed",
            Self::Manifest => "import matching failed",
            Self::TemporaryUrl => "failed to get download URL",
            Self::AudioNotReady => {
                "audio download link is not ready yet; will try again on next sync"
            }
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

    /// Download `file` from `url` into `destination_dir`, deriving the audio extension from the
    /// presigned URL or response `Content-Type` rather than hardcoding `.opus`.
    fn download_for_file(
        &mut self,
        url: &str,
        destination_dir: &Path,
        file: &PlaudFile,
    ) -> Result<PathBuf, PlaudFailureKind> {
        let derived_ext = audio_extension_from_url(url);
        let destination = destination_dir.join(destination_name_with_extension(
            file,
            derived_ext.as_deref(),
        ));
        self.download(url, &destination)?;
        Ok(destination)
    }
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
    pub source_instant: Option<Timestamp>,
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
    /// A later file was not started because the attempt was stopping.
    pub stopped: bool,
}

struct ResolvedImportTimestamp {
    timestamp: String,
    source_instant: Timestamp,
    used_fallback: bool,
    downloaded_bytes: Option<u64>,
}

/// Named Plaud sync failure with no transport detail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PlaudSyncError {
    MissingCredential,
    Operation(PlaudFailureKind),
    State { message: String },
}

impl PlaudSyncError {
    #[must_use]
    pub fn render(&self, locale: &str) -> String {
        match self {
            Self::MissingCredential => solstone_core_cli_copy::render_for(
                locale,
                "journal.cli.importer.plaud.sync.missing.credential",
                &[],
            ),
            Self::Operation(kind) => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.importer.plaud.sync.operation",
                [("detail", &kind.render(locale))],
            ),
            Self::State { message } => solstone_core_cli_copy::display_for(
                locale,
                "journal.cli.importer.plaud.sync.state.failed",
                [("message", message)],
            ),
        }
    }
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
        stopped: false,
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
    let mut stopped = false;
    let mut first_error_code: Option<&'static str> = None;
    let mut used_fallback_timestamps = HashSet::new();
    for file in catalogued.available {
        if !crate::work_continues() {
            stopped = true;
            break;
        }
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
            FiledPlaud::Stopped => {
                stopped = true;
                break;
            }
            FiledPlaud::Imported(resolved) => {
                let selected = resolved.timestamp.clone();
                let imported_at = seams.preview.clock.now();
                entry.insert("status".to_owned(), Value::String("imported".to_owned()));
                entry.insert(
                    "import_timestamp".to_owned(),
                    Value::String(resolved.timestamp),
                );
                if let Some(size) = resolved.downloaded_bytes
                    && size > 0
                {
                    entry.insert("filesize".to_owned(), Value::from(size));
                }
                if resolved.used_fallback {
                    entry.insert("import_timestamp_fallback".to_owned(), Value::Bool(true));
                }
                entry.insert("imported_at".to_owned(), Value::String(imported_at.clone()));
                entry.remove("last_error");
                catalogued
                    .state
                    .root_mut()
                    .insert("last_added".to_owned(), Value::String(imported_at));
                downloaded += 1;
                seams
                    .preview
                    .state_writer
                    .checkpoint(&request.journal_root, &catalogued.state)
                    .map_err(state_error)?;
                if let Err(error) = crate::audio_originals::finish(&request.journal_root, &selected)
                {
                    log::warn!("Plaud audio cleanup {selected}: {error}");
                }
            }
            FiledPlaud::Failed(kind) => {
                entry.insert("status".to_owned(), Value::String("available".to_owned()));
                entry.insert(
                    "last_error".to_owned(),
                    Value::String(kind.message().to_owned()),
                );
                if first_error_code.is_none() {
                    first_error_code = Some(crate::plaud::attention_code_for_failure(kind));
                }
                errors.push(format!(
                    "{}: {}",
                    file.filename,
                    kind.render(solstone_core_cli_copy::cli_locale())
                ));
            }
        }
    }
    if let Some(code) = first_error_code {
        catalogued
            .state
            .root_mut()
            .insert("last_error_code".to_owned(), Value::String(code.to_owned()));
    } else if !stopped {
        catalogued.state.root_mut().remove("last_error_code");
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
        stopped,
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
    // The record is the sync's memory of what it brought in: one it can't read
    // would forget every recording the owner deleted, so nothing syncs until it reads.
    let mut state = match read_sync_state(&request.journal_root, BackendName::Plaud) {
        SyncStateRead::Loaded(state) => state,
        SyncStateRead::Absent => SyncState::empty(BackendName::Plaud),
        SyncStateRead::Unreadable { .. } => {
            return Err(PlaudSyncError::State {
                message:
                    "the record of what this sync brought in can't be read, so nothing was synced"
                        .to_owned(),
            });
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
        if file.filesize > 0 || !object.contains_key("filesize") {
            object.insert("filesize".to_owned(), Value::from(file.filesize));
        }
        if existed {
            // What the sync recorded as imported stays imported, whatever the owner has
            // since deleted: a sync never brings it back. Importing the downloaded file
            // again does.
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

enum FiledPlaud {
    Imported(ResolvedImportTimestamp),
    Failed(PlaudFailureKind),
    Stopped,
}

fn import_file(
    file: &PlaudFile,
    token: &str,
    journal_root: &Path,
    seams: &mut PlaudSaveSeams<'_>,
    used_fallback_timestamps: &mut HashSet<String>,
) -> FiledPlaud {
    let now = seams.preview.clock.now();
    let zone = solstone_core_journal_config::owner_zone(journal_root);
    let Some(resolved) = import_timestamp(file.start_time, &now, &zone, used_fallback_timestamps)
    else {
        return FiledPlaud::Failed(PlaudFailureKind::Pipeline);
    };
    let Ok(semantic) = crate::validate_timestamp(&resolved.timestamp) else {
        return FiledPlaud::Failed(PlaudFailureKind::Pipeline);
    };
    // The device time stays the source timestamp. The directory id is whatever
    // the allocator can claim, so two recordings in one second do not share it.
    let Ok(bound) = crate::bind_import_record(journal_root, &semantic, None) else {
        return FiledPlaud::Failed(PlaudFailureKind::Download);
    };
    let selected = bound.import_id.as_str().to_owned();
    let destination_dir = journal_root.join("imports").join(&selected);
    let url = match seams.download.temporary_url(token, &file.id) {
        Ok(url) => url,
        Err(kind) => return FiledPlaud::Failed(kind),
    };
    let destination = match seams
        .download
        .download_for_file(&url, &destination_dir, file)
    {
        Ok(path) => path,
        Err(kind) => return FiledPlaud::Failed(kind),
    };
    let downloaded_bytes = std::fs::metadata(&destination).ok().map(|meta| meta.len());
    if destination.is_file() {
        let registered = (|| -> Result<(), String> {
            let hash = crate::hash_source(&destination).map_err(|error| error.to_string())?;
            let mut metadata = crate::read_import_metadata(journal_root, &selected)
                .map_err(|error| error.to_string())?;
            crate::originals::record_staged_upload(&mut metadata, &destination);
            metadata.insert("source_hash".to_owned(), Value::String(hash.into_inner()));
            metadata.insert("file_path".to_owned(), serde_json::json!(destination));
            metadata.insert(
                "original_filename".to_owned(),
                Value::String(file.filename.clone()),
            );
            metadata.insert("plaud_file_id".to_owned(), Value::String(file.id.clone()));
            crate::write_import_metadata(journal_root, &selected, &metadata)
                .map_err(|error| error.to_string())?;
            crate::originals::capture_choice(journal_root, &selected, "plaud")
        })();
        if let Err(error) = registered {
            log::warn!("Plaud audio registration {selected}: {error}");
            return FiledPlaud::Failed(PlaudFailureKind::Pipeline);
        }
    }
    // The download and its admitted copy stay. The next audio unit does not start.
    if !crate::work_continues() {
        return FiledPlaud::Stopped;
    }
    match seams.pipeline.import_one(PipelineImportRequest {
        source: &destination,
        source_kind: "plaud",
        timestamp: Some(&selected),
        source_instant: Some(resolved.source_instant),
        auto: PipelineAuto::Enabled,
    }) {
        Ok(PipelineOutcome::Imported) => FiledPlaud::Imported(ResolvedImportTimestamp {
            timestamp: selected,
            source_instant: resolved.source_instant,
            used_fallback: resolved.used_fallback,
            downloaded_bytes,
        }),
        Ok(PipelineOutcome::Skipped { .. })
        | Ok(PipelineOutcome::NoResult)
        | Ok(PipelineOutcome::Unrecognized)
        | Err(_) => FiledPlaud::Failed(PlaudFailureKind::Pipeline),
    }
}

/// The device's recording start as a wall time in the owner zone.
fn import_timestamp(
    start_time: f64,
    now: &str,
    zone: &TimeZone,
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
    // A start past the year 9999 is later than any clock reading: it takes
    // the fallback below, as any start too far ahead does.
    let device_time = Timestamp::new(seconds as i64, 0).ok();
    let now = solstone_core_time::parse_rfc3339(now).ok()?;
    if let Some(device_time) = device_time
        && device_time.duration_since(now.timestamp()) <= SignedDuration::from_hours(48)
    {
        return Some(ResolvedImportTimestamp {
            timestamp: device_time
                .to_zoned_datetime(zone.clone())
                .strftime("%Y%m%d_%H%M%S")
                .to_string(),
            source_instant: device_time,
            used_fallback: false,
            downloaded_bytes: None,
        });
    }

    let mut fallback_time = now.with_time_zone(zone.clone());
    loop {
        let timestamp = fallback_time.strftime("%Y%m%d_%H%M%S").to_string();
        if used_fallback_timestamps.insert(timestamp.clone()) {
            return Some(ResolvedImportTimestamp {
                timestamp,
                source_instant: fallback_time.timestamp(),
                used_fallback: true,
                downloaded_bytes: None,
            });
        }
        fallback_time = fallback_time
            .checked_add(SignedDuration::from_secs(1))
            .ok()?;
    }
}

/// Extract a known audio extension (with leading dot, lowercased) from a presigned URL path.
#[must_use]
pub fn audio_extension_from_url(url: &str) -> Option<String> {
    let without_query = url.split(['?', '#']).next().unwrap_or(url);
    let last_segment = without_query.rsplit('/').next()?;
    let ext = Path::new(last_segment)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "mp3" | "m4a" | "wav" | "ogg" | "opus" | "flac" | "aac" | "webm" | "mp4" => {
            Some(format!(".{ext}"))
        }
        _ => None,
    }
}

/// Derive a known audio extension (with leading dot) from an HTTP `Content-Type` header.
#[must_use]
pub fn audio_extension_from_content_type(content_type: &str) -> Option<String> {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or(content_type)
        .trim()
        .to_ascii_lowercase();
    let ext = match mime.as_str() {
        "audio/mpeg" | "audio/mp3" => ".mp3",
        "audio/mp4" | "audio/x-m4a" | "audio/m4a" => ".m4a",
        "audio/wav" | "audio/x-wav" | "audio/wave" => ".wav",
        "audio/ogg" | "audio/opus" => ".opus",
        "audio/flac" | "audio/x-flac" => ".flac",
        "audio/aac" => ".aac",
        "audio/webm" => ".webm",
        _ => return None,
    };
    Some(ext.to_owned())
}

#[must_use]
pub fn destination_name_with_extension(file: &PlaudFile, preferred_ext: Option<&str>) -> String {
    let extension = preferred_ext
        .filter(|ext| ext.starts_with('.') && ext.len() > 1)
        .map(str::to_owned)
        .or_else(|| {
            Path::new(&file.fullname)
                .extension()
                .filter(|extension| !extension.is_empty())
                .map(|extension| format!(".{}", extension.to_string_lossy()))
        })
        .unwrap_or_else(|| ".opus".to_owned());
    format!("{}{}", sanitize_filename(&file.filename), extension)
}

/// A Plaud display name as a safe file name, as Plaud downloads and matching both spell it.
pub fn sanitize_filename(filename: &str) -> String {
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
