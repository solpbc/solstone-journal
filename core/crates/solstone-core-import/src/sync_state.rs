// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Typed private state and dispatch for importer sync catalogues.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use chrono::Utc;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use solstone_core_body_ingest::{OuraSyncOptions, sync_oura};
use solstone_core_journal_io::{JsonWriteOptions, create_directory_with_mode, write_json};

use crate::consent_gate::{
    ScheduledSyncGuidance, body_error_to_import, read_oura_scheduled_sync_guidance,
};
use crate::sync_audio::{AudioSyncOptions, sync_audio};
use crate::sync_obsidian::{ObsidianSyncOptions, sync_obsidian};
use crate::sync_plaud::{LivePlaudHttp, PlaudSyncOptions, sync_plaud_with_http};
use crate::{AutoTimestamp, ImportError};

/// The ordered importer sync backends, including native Oura.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SyncBackend {
    Plaud,
    Obsidian,
    Audio,
    Oura,
}

impl SyncBackend {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plaud => "plaud",
            Self::Obsidian => "obsidian",
            Self::Audio => "audio",
            Self::Oura => "oura",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        Some(match name {
            "plaud" => Self::Plaud,
            "obsidian" => Self::Obsidian,
            "audio" => Self::Audio,
            "oura" => Self::Oura,
            _ => return None,
        })
    }
}

/// File-backed sync state owners. Oura owns a separate native cursor.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileSyncBackend {
    Plaud,
    Obsidian,
    Audio,
}

impl FileSyncBackend {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Plaud => "plaud",
            Self::Obsidian => "obsidian",
            Self::Audio => "audio",
        }
    }

    #[must_use]
    pub const fn sync_backend(self) -> SyncBackend {
        match self {
            Self::Plaud => SyncBackend::Plaud,
            Self::Obsidian => SyncBackend::Obsidian,
            Self::Audio => SyncBackend::Audio,
        }
    }
}

/// One exact file-backend state envelope. Item objects retain every reference field.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileSyncState {
    pub backend: FileSyncBackend,
    pub files: BTreeMap<String, Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_path: Option<String>,
}

impl FileSyncState {
    #[must_use]
    pub fn empty(backend: FileSyncBackend, source_path: Option<String>) -> Self {
        Self {
            backend,
            files: BTreeMap::new(),
            last_sync: None,
            source_path,
        }
    }

    pub fn stamp(&mut self) {
        self.last_sync = Some(Utc::now().to_rfc3339());
    }
}

/// The caller-owned sync request after argv processing.
pub struct SyncRequest<'a> {
    pub journal: &'a Path,
    pub backend: &'a str,
    pub save: bool,
    pub source_path: Option<&'a Path>,
    pub window_days: Option<u64>,
    pub confirm_body_save: bool,
    pub scheduled: bool,
    pub force: bool,
    pub auto: AutoTimestamp,
    pub plaud_access_token: Option<&'a str>,
}

/// One later-owned import action.
pub struct SyncActionRequest<'a> {
    pub backend: SyncBackend,
    pub item_key: &'a str,
    pub item_name: &'a str,
    pub source_path: Option<&'a Path>,
    pub metadata: SyncActionMetadata<'a>,
}

/// Backend-specific source metadata passed across the unfinished save boundary.
pub enum SyncActionMetadata<'a> {
    Plaud(&'a Map<String, Value>),
    Obsidian(&'a Map<String, Value>),
    Audio(&'a Map<String, Value>),
}

/// An action failure safe to persist and show to the owner.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncActionFailure {
    pub message: String,
}

/// Explicit external action boundary for the unfinished save half.
pub struct SyncActionSeams<A> {
    pub per_item_action: A,
}

/// A completed or failed item action.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SyncItemFailure {
    pub item: String,
    pub reason: String,
}

/// Native Oura data returned without exposing native private report fields.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OuraSyncSummary {
    pub rows: u64,
    pub days: Vec<String>,
    pub pages: u64,
    pub skipped: bool,
}

/// Result of one sync operation.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct SyncReport {
    pub total: u64,
    pub imported: u64,
    pub available: u64,
    pub skipped: u64,
    pub downloaded: u64,
    pub failures: Vec<SyncItemFailure>,
    pub oura: Option<OuraSyncSummary>,
    pub scheduled_guidance: Option<ScheduledSyncGuidance>,
}

const BACKENDS: [SyncBackend; 4] = [
    SyncBackend::Plaud,
    SyncBackend::Obsidian,
    SyncBackend::Audio,
    SyncBackend::Oura,
];

#[must_use]
pub const fn available_sync_backends() -> &'static [SyncBackend] {
    &BACKENDS
}

pub fn load_sync_state(
    journal: &Path,
    backend: FileSyncBackend,
) -> Result<Option<FileSyncState>, ImportError> {
    let path = state_path(journal, backend);
    let bytes = match fs::read(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => {
            return Err(ImportError::SyncStateRead {
                path,
                message: error.to_string(),
            });
        }
    };
    let state: FileSyncState =
        serde_json::from_slice(&bytes).map_err(|error| ImportError::SyncStateRead {
            path: path.clone(),
            message: error.to_string(),
        })?;
    if state.backend != backend {
        return Err(ImportError::SyncStateRead {
            path,
            message: "state backend does not match its filename".to_owned(),
        });
    }
    Ok(Some(state))
}

pub fn write_sync_state(journal: &Path, state: &FileSyncState) -> Result<(), ImportError> {
    let imports = journal.join("imports");
    create_directory_with_mode(&imports, 0o700).map_err(|error| ImportError::SyncStateWrite {
        path: imports.clone(),
        message: error.to_string(),
    })?;
    let path = state_path(journal, state.backend);
    write_json(
        &path,
        state,
        JsonWriteOptions {
            mode: Some(0o600),
            ..JsonWriteOptions::default()
        },
    )
    .map_err(|error| ImportError::SyncStateWrite {
        path,
        message: error.to_string(),
    })
}

#[must_use]
pub fn state_path(journal: &Path, backend: FileSyncBackend) -> PathBuf {
    journal
        .join("imports")
        .join(format!("{}.json", backend.name()))
}

/// Dispatches a typed sync request without parsing argv or rendering output.
pub fn dispatch_sync<A>(
    request: &SyncRequest<'_>,
    seams: &mut SyncActionSeams<A>,
) -> Result<SyncReport, ImportError>
where
    A: for<'a> FnMut(SyncActionRequest<'a>) -> Result<(), SyncActionFailure>,
{
    match SyncBackend::from_name(request.backend) {
        Some(SyncBackend::Plaud) => {
            let token = request
                .plaud_access_token
                .ok_or_else(|| ImportError::Refusal {
                    kind: "plaud_access_token_missing",
                    exit_code: 65,
                    message: "Plaud sync requires an owner-supplied access token".to_owned(),
                })?;
            let options = PlaudSyncOptions {
                journal: request.journal,
                save: request.save,
                access_token: token,
            };
            let mut http = LivePlaudHttp::new();
            sync_plaud_with_http(&options, &mut http, seams)
        }
        Some(SyncBackend::Obsidian) => sync_obsidian(
            &ObsidianSyncOptions {
                journal: request.journal,
                save: request.save,
                source_path: request.source_path,
                force: request.force,
            },
            seams,
        ),
        Some(SyncBackend::Audio) => sync_audio(
            &AudioSyncOptions {
                journal: request.journal,
                save: request.save,
                source_path: request.source_path,
                force: request.force,
                auto: request.auto.clone(),
            },
            seams,
        ),
        Some(SyncBackend::Oura) => {
            let native = sync_oura(
                request.journal,
                &OuraSyncOptions {
                    save: request.save,
                    confirm_body_save: request.confirm_body_save,
                    scheduled: request.scheduled,
                    window_days: request.window_days,
                    today: None,
                },
            )
            .map_err(|error| body_error_to_import(error, request.scheduled))?;
            let scheduled_guidance = if request.scheduled {
                read_oura_scheduled_sync_guidance(request.journal)?
            } else {
                None
            };
            Ok(SyncReport {
                total: native.rows(),
                imported: native.rows(),
                available: 0,
                skipped: u64::from(native.quiet_run()),
                downloaded: 0,
                failures: Vec::new(),
                oura: Some(OuraSyncSummary {
                    rows: native.rows(),
                    days: native.days().to_vec(),
                    pages: native.pages(),
                    skipped: native.quiet_run(),
                }),
                scheduled_guidance,
            })
        }
        None => Err(ImportError::Refusal {
            kind: "unknown_sync_backend",
            exit_code: 2,
            message: format!("unknown sync backend: {}", request.backend),
        }),
    }
}

pub(crate) fn run_actions<A>(
    state: &mut FileSyncState,
    save: bool,
    journal: &Path,
    seams: &mut SyncActionSeams<A>,
    source_paths: &BTreeMap<String, PathBuf>,
) -> Result<SyncReport, ImportError>
where
    A: for<'a> FnMut(SyncActionRequest<'a>) -> Result<(), SyncActionFailure>,
{
    let mut report = report_from_state(state);
    if !save {
        return Ok(report);
    }
    let backend = state.backend.sync_backend();
    let available: Vec<String> = state
        .files
        .iter()
        .filter(|(_, entry)| entry.get("status").and_then(Value::as_str) == Some("available"))
        .map(|(key, _)| key.clone())
        .collect();
    for key in available {
        let (name, metadata) = {
            let entry = state.files.get(&key).expect("key collected from state");
            (
                entry
                    .get("filename")
                    .and_then(Value::as_str)
                    .unwrap_or(&key)
                    .to_owned(),
                entry
                    .as_object()
                    .expect("sync state entries are objects")
                    .clone(),
            )
        };
        let source_path = source_paths.get(&key).map(PathBuf::as_path);
        let action = (seams.per_item_action)(SyncActionRequest {
            backend,
            item_key: &key,
            item_name: &name,
            source_path,
            metadata: action_metadata(backend, &metadata),
        });
        match action {
            Ok(()) => {
                let entry = state.files.get_mut(&key).expect("state key still exists");
                entry["status"] = Value::String("imported".to_owned());
                entry["imported_at"] = Value::String(Utc::now().to_rfc3339());
                state.stamp();
                write_sync_state(journal, state)?;
                report.downloaded += 1;
            }
            Err(error) => {
                let entry = state.files.get_mut(&key).expect("state key still exists");
                entry["last_error"] = Value::String(error.message.clone());
                report.failures.push(SyncItemFailure {
                    item: name,
                    reason: error.message,
                });
            }
        }
    }
    state.stamp();
    write_sync_state(journal, state)?;
    let mut final_report = report_from_state(state);
    final_report.downloaded = report.downloaded;
    final_report.failures = report.failures;
    Ok(final_report)
}

fn action_metadata(backend: SyncBackend, metadata: &Map<String, Value>) -> SyncActionMetadata<'_> {
    match backend {
        SyncBackend::Plaud => SyncActionMetadata::Plaud(metadata),
        SyncBackend::Obsidian => SyncActionMetadata::Obsidian(metadata),
        SyncBackend::Audio => SyncActionMetadata::Audio(metadata),
        SyncBackend::Oura => unreachable!("Oura does not use file action seams"),
    }
}

pub(crate) fn report_from_state(state: &FileSyncState) -> SyncReport {
    let mut report = SyncReport {
        total: state.files.len() as u64,
        ..SyncReport::default()
    };
    for entry in state.files.values() {
        match entry.get("status").and_then(Value::as_str) {
            Some("imported") => report.imported += 1,
            Some("available") => report.available += 1,
            Some("skipped") => report.skipped += 1,
            _ => {}
        }
    }
    report
}
