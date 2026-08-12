// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Local audio-folder sync behind scanner, probe, and pipeline seams.

use std::fmt;
use std::marker::PhantomData;
use std::path::{Path, PathBuf};

use serde_json::{Map, Value};

use crate::contract::{AudioAuto, SyncPreviewRequest, SyncSaveRequest};
use crate::sync_plaud::{
    ImportPipeline, PipelineAuto, PipelineImportRequest, PipelineOutcome, SyncClock,
};
use crate::sync_state::{BackendName, SyncState, SyncStateRead, read_sync_state, write_sync_state};

/// One audio candidate supplied by the caller-owned directory scanner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioCandidate {
    pub relative_path: String,
    pub source: PathBuf,
    pub filename: String,
    pub filesize: u64,
    pub source_hash: String,
}

/// Caller-owned recursive audio enumeration.
pub trait DirectoryScanner {
    fn audio_candidates(&self, root: &Path) -> Result<Vec<AudioCandidate>, String>;
}

/// Caller-owned ffprobe-equivalent duration query.
pub trait AudioProbe {
    fn duration_seconds(&self, source: &Path) -> Result<Option<f64>, String>;
}

/// Caller-owned manifest lookup.
pub trait ManifestLookup {
    fn imported_hash(&self, source_hash: &str) -> bool;
}

/// Checkpoint seam, allowing progress ordering to be tested without the pipeline.
pub trait AudioStateWriter {
    fn checkpoint(&mut self, journal_root: &Path, state: &SyncState) -> Result<(), String>;
}

/// Production state publication through the private atomic state writer.
pub struct FilesystemAudioStateWriter;

impl AudioStateWriter for FilesystemAudioStateWriter {
    fn checkpoint(&mut self, journal_root: &Path, state: &SyncState) -> Result<(), String> {
        write_sync_state(journal_root, state).map_err(|error| error.to_string())
    }
}

/// Named seams that preview may use. They contain no import-pipeline authority.
pub struct AudioPreviewSeams<'a> {
    pub scanner: &'a dyn DirectoryScanner,
    pub probe: &'a dyn AudioProbe,
    pub manifests: &'a dyn ManifestLookup,
    pub clock: &'a dyn SyncClock,
    pub state_writer: &'a mut dyn AudioStateWriter,
}

/// Named save seams, extending preview authority with the import pipeline.
pub struct AudioSaveSeams<'a> {
    pub preview: AudioPreviewSeams<'a>,
    pub pipeline: &'a mut dyn ImportPipeline,
}

/// Audio sync request with a type-level preview/save marker.
pub struct AudioSyncRequest<M> {
    pub journal_root: PathBuf,
    pub source_path: PathBuf,
    pub force: bool,
    pub auto: AudioAuto,
    marker: PhantomData<M>,
}

impl<M> AudioSyncRequest<M> {
    #[must_use]
    pub fn new(journal_root: PathBuf, source_path: PathBuf, force: bool, auto: AudioAuto) -> Self {
        Self {
            journal_root,
            source_path,
            force,
            auto,
            marker: PhantomData,
        }
    }
}

/// One item-level outcome, retained even when later items fail.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AudioItemOutcome {
    pub relative_path: String,
    pub imported: bool,
    pub checkpointed: bool,
    pub error: Option<String>,
}

/// Audio sync's state and item-level outcome surface.
#[derive(Clone, Debug, PartialEq)]
pub struct AudioSyncOutcome {
    pub state: SyncState,
    pub downloaded: u64,
    pub errors: Vec<String>,
    pub items: Vec<AudioItemOutcome>,
}

/// Named audio sync failure.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum AudioSyncError {
    MissingSource,
    Scan(String),
    State(String),
}

impl fmt::Display for AudioSyncError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingSource => formatter.write_str("Audio sync requires a source directory"),
            Self::Scan(message) => write!(formatter, "Audio scan failed: {message}"),
            Self::State(message) => write!(formatter, "Audio state failed: {message}"),
        }
    }
}

impl std::error::Error for AudioSyncError {}

pub fn sync_audio_preview(
    request: &AudioSyncRequest<SyncPreviewRequest>,
    seams: &mut AudioPreviewSeams<'_>,
) -> Result<AudioSyncOutcome, AudioSyncError> {
    let catalogued = catalogue(
        request,
        seams.scanner,
        seams.probe,
        seams.manifests,
        seams.clock,
    )?;
    seams
        .state_writer
        .checkpoint(&request.journal_root, &catalogued.state)
        .map_err(AudioSyncError::State)?;
    Ok(AudioSyncOutcome {
        state: catalogued.state,
        downloaded: 0,
        errors: catalogued.errors,
        items: Vec::new(),
    })
}

pub fn sync_audio_save(
    request: &AudioSyncRequest<SyncSaveRequest>,
    seams: &mut AudioSaveSeams<'_>,
) -> Result<AudioSyncOutcome, AudioSyncError> {
    let mut catalogued = catalogue(
        request,
        seams.preview.scanner,
        seams.preview.probe,
        seams.preview.manifests,
        seams.preview.clock,
    )?;
    let mut downloaded = 0;
    let mut items = Vec::new();
    for candidate in catalogued.available {
        let result = seams.pipeline.import_one(PipelineImportRequest {
            source: &candidate.source,
            source_kind: "audio",
            timestamp: None,
            auto: pipeline_auto(&request.auto),
        });
        let entry = catalogued
            .state
            .files_mut()
            .get_mut(&candidate.relative_path)
            .and_then(Value::as_object_mut)
            .expect("catalogued audio exists");
        let (imported, error) = match result {
            Ok(PipelineOutcome::Imported) => {
                entry.insert("status".to_owned(), Value::String("imported".to_owned()));
                entry.insert(
                    "imported_at".to_owned(),
                    Value::String(seams.preview.clock.now()),
                );
                entry.remove("last_error");
                downloaded += 1;
                (true, None)
            }
            Ok(PipelineOutcome::Skipped { reason }) => {
                (false, Some(format!("import skipped: {reason}")))
            }
            Ok(PipelineOutcome::NoResult) => (false, Some("import returned no result".to_owned())),
            Ok(PipelineOutcome::Unrecognized) => (
                false,
                Some("import returned unrecognized result".to_owned()),
            ),
            Err(message) => (false, Some(message)),
        };
        if let Some(message) = &error {
            entry.insert("status".to_owned(), Value::String("available".to_owned()));
            entry.insert("last_error".to_owned(), Value::String(message.clone()));
            catalogued
                .errors
                .push(format!("{}: {message}", candidate.relative_path));
        }
        seams
            .preview
            .state_writer
            .checkpoint(&request.journal_root, &catalogued.state)
            .map_err(AudioSyncError::State)?;
        items.push(AudioItemOutcome {
            relative_path: candidate.relative_path,
            imported,
            checkpointed: true,
            error,
        });
    }
    seams
        .preview
        .state_writer
        .checkpoint(&request.journal_root, &catalogued.state)
        .map_err(AudioSyncError::State)?;
    Ok(AudioSyncOutcome {
        state: catalogued.state,
        downloaded,
        errors: catalogued.errors,
        items,
    })
}

struct CataloguedAudio {
    state: SyncState,
    available: Vec<AudioCandidate>,
    errors: Vec<String>,
}

fn catalogue<M>(
    request: &AudioSyncRequest<M>,
    scanner: &dyn DirectoryScanner,
    probe: &dyn AudioProbe,
    manifests: &dyn ManifestLookup,
    clock: &dyn SyncClock,
) -> Result<CataloguedAudio, AudioSyncError> {
    if request.source_path.as_os_str().is_empty() {
        return Err(AudioSyncError::MissingSource);
    }
    let mut state = match read_sync_state(&request.journal_root, BackendName::Audio) {
        SyncStateRead::Loaded(state) => state,
        SyncStateRead::Absent | SyncStateRead::Unreadable { .. } => {
            SyncState::empty(BackendName::Audio)
        }
    };
    if request.force {
        state
            .root_mut()
            .insert("files".to_owned(), Value::Object(Map::new()));
    }
    for entry in state
        .files_mut()
        .values_mut()
        .filter_map(Value::as_object_mut)
    {
        if entry.get("status") != Some(&Value::String("available".to_owned())) {
            continue;
        }
        let Some(hash) = entry.get("hash").and_then(Value::as_str) else {
            continue;
        };
        if manifests.imported_hash(hash) {
            entry.insert("status".to_owned(), Value::String("imported".to_owned()));
            entry.insert("imported_at".to_owned(), Value::String(clock.now()));
            entry.remove("last_error");
            entry.remove("skip_reason");
        }
    }
    let candidates = scanner
        .audio_candidates(&request.source_path)
        .map_err(AudioSyncError::Scan)?;
    if candidates.is_empty() {
        return Err(AudioSyncError::MissingSource);
    }
    let mut seen = Vec::new();
    let mut available = Vec::new();
    let mut errors = Vec::new();
    for candidate in candidates {
        seen.push(candidate.relative_path.clone());
        let entry = state
            .files_mut()
            .entry(candidate.relative_path.clone())
            .or_insert_with(|| Value::Object(Map::new()));
        let entry = entry.as_object_mut().expect("sync-state files are objects");
        entry.insert(
            "filename".to_owned(),
            Value::String(candidate.filename.clone()),
        );
        entry.insert("filesize".to_owned(), Value::from(candidate.filesize));
        entry.insert(
            "hash".to_owned(),
            Value::String(candidate.source_hash.clone()),
        );
        if manifests.imported_hash(&candidate.source_hash) {
            entry.insert("status".to_owned(), Value::String("imported".to_owned()));
            entry.insert("imported_at".to_owned(), Value::String(clock.now()));
            entry.remove("last_error");
            entry.remove("skip_reason");
            continue;
        }
        match probe.duration_seconds(&candidate.source) {
            Ok(Some(duration)) if duration >= 30.0 => {
                entry.insert("status".to_owned(), Value::String("available".to_owned()));
                entry.insert("duration".to_owned(), Value::from(duration));
                entry.remove("last_error");
                entry.remove("skip_reason");
                available.push(candidate);
            }
            Ok(Some(duration)) => {
                entry.insert("status".to_owned(), Value::String("skipped".to_owned()));
                entry.insert("duration".to_owned(), Value::from(duration));
                entry.insert(
                    "skip_reason".to_owned(),
                    Value::String("too_short".to_owned()),
                );
                entry.remove("last_error");
            }
            Ok(None) | Err(_) => {
                entry.insert("status".to_owned(), Value::String("unreadable".to_owned()));
                entry.remove("duration");
                entry.remove("last_error");
                entry.remove("skip_reason");
                errors.push(format!(
                    "{}: could not read audio (probe failed)",
                    candidate.relative_path
                ));
            }
        }
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
        Value::String(request.source_path.display().to_string()),
    );
    state
        .root_mut()
        .insert("last_sync".to_owned(), Value::String(clock.now()));
    Ok(CataloguedAudio {
        state,
        available,
        errors,
    })
}

fn pipeline_auto(auto: &AudioAuto) -> PipelineAuto<'_> {
    match auto {
        AudioAuto::Enabled => PipelineAuto::Enabled,
        AudioAuto::Disabled => PipelineAuto::Disabled,
        AudioAuto::Value(value) => PipelineAuto::Value(value),
    }
}
