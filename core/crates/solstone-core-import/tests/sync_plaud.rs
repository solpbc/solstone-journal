// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::RefCell;
use std::collections::BTreeMap;
use std::path::Path;

use solstone_core_import::contract::{SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::sync_plaud::{
    ImportPipeline, PipelineAuto, PipelineImportRequest, PipelineOutcome, PlaudCatalogue,
    PlaudCredential, PlaudDownload, PlaudFailureKind, PlaudFile, PlaudManifestLookup,
    PlaudPreviewSeams, PlaudSaveSeams, PlaudStateWriter, PlaudSyncError, PlaudSyncRequest,
    SyncClock, sync_plaud_preview, sync_plaud_save,
};
use solstone_core_import::{BackendName, SyncState, state_path};
use tempfile::TempDir;

struct Credential;
impl PlaudCredential for Credential {
    fn access_token(&self) -> Option<&str> {
        Some("credential-not-for-state")
    }
}

struct Clock;
impl SyncClock for Clock {
    fn now(&self) -> String {
        "2026-08-11T12:00:00+00:00".to_owned()
    }
}

struct Catalogue {
    files: Vec<PlaudFile>,
    fail: bool,
}
impl PlaudCatalogue for Catalogue {
    fn list_files(&mut self, _: &str) -> Result<Vec<PlaudFile>, PlaudFailureKind> {
        if self.fail {
            Err(PlaudFailureKind::Catalogue)
        } else {
            Ok(self.files.clone())
        }
    }
}

#[derive(Default)]
struct Downloader {
    downloads: Vec<String>,
    fail_download_for: Option<&'static str>,
}
impl PlaudDownload for Downloader {
    fn temporary_url(&mut self, _: &str, file_id: &str) -> Result<String, PlaudFailureKind> {
        Ok(format!(
            "https://example.invalid/{file_id}?signature=secret"
        ))
    }

    fn download(&mut self, url: &str, _: &Path) -> Result<(), PlaudFailureKind> {
        let file_id = url
            .strip_prefix("https://example.invalid/")
            .and_then(|value| value.split('?').next())
            .unwrap();
        if self.fail_download_for == Some(file_id) {
            return Err(PlaudFailureKind::Download);
        }
        self.downloads.push(file_id.to_owned());
        Ok(())
    }
}

struct Matches(BTreeMap<String, String>);
impl PlaudManifestLookup for Matches {
    fn matching_imports(&self, _: &[PlaudFile]) -> Result<BTreeMap<String, String>, String> {
        Ok(self.0.clone())
    }
}

#[derive(Default)]
struct Writer {
    checkpoints: Vec<SyncState>,
}
impl PlaudStateWriter for Writer {
    fn checkpoint(&mut self, _: &Path, state: &SyncState) -> Result<(), String> {
        self.checkpoints.push(state.clone());
        Ok(())
    }
}

struct PanicPipeline;
impl ImportPipeline for PanicPipeline {
    fn import_one(&mut self, _: PipelineImportRequest<'_>) -> Result<PipelineOutcome, String> {
        panic!("preview has no pipeline authority")
    }
}

#[derive(Default)]
struct ImportedPipeline {
    calls: RefCell<Vec<(Option<String>, PipelineAuto<'static>)>>,
}
impl ImportPipeline for ImportedPipeline {
    fn import_one(
        &mut self,
        request: PipelineImportRequest<'_>,
    ) -> Result<PipelineOutcome, String> {
        let auto = match request.auto {
            PipelineAuto::Enabled => PipelineAuto::Enabled,
            PipelineAuto::Disabled => PipelineAuto::Disabled,
            PipelineAuto::Value(_) => unreachable!("Plaud always enables auto"),
        };
        self.calls
            .borrow_mut()
            .push((request.timestamp.map(str::to_owned), auto));
        Ok(PipelineOutcome::Imported)
    }
}

#[test]
fn preview_matches_before_cataloguing_and_has_no_download_or_pipeline_authority() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![file("matched", 1_725_000_000_000, 60_000)],
        fail: false,
    };
    let matches = Matches(BTreeMap::from([(
        "matched".to_owned(),
        "20240801_010203".to_owned(),
    )]));
    let mut writer = Writer::default();
    let mut seams = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut writer,
    };
    let request = PlaudSyncRequest::<SyncPreviewRequest>::new(tree.path().to_path_buf());

    let outcome = sync_plaud_preview(&request, &mut seams).unwrap();
    let entry = &outcome.state.root()["files"]["matched"];
    assert_eq!(entry["status"], "imported");
    assert_eq!(entry["import_timestamp"], "20240801_010203");
    assert_eq!(entry["matched_at"], "2026-08-11T12:00:00+00:00");
    assert_eq!(entry["fullname"], "matched.opus");
    assert_eq!(entry["start_time"], 1_725_000_000_000_i64);
    assert_eq!(entry["duration"], 60_000_i64);
    assert_eq!(entry["is_trash"], false);
    assert_eq!(outcome.downloaded, 0);
    assert_eq!(writer.checkpoints.len(), 1);
    assert!(!format!("{outcome:?}").contains("credential-not-for-state"));
}

#[test]
fn available_entry_is_promoted_when_a_later_manifest_match_appears() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![file("later", 1_725_000_000_000, 60_000)],
        fail: false,
    };
    let mut state = SyncState::empty(BackendName::Plaud);
    state.files_mut().insert(
        "later".to_owned(),
        serde_json::json!({"status": "available", "filename": "old", "filesize": 1}),
    );
    std::fs::create_dir_all(tree.path().join("imports")).unwrap();
    std::fs::write(
        state_path(tree.path(), BackendName::Plaud),
        serde_json::to_vec(&state.root()).unwrap(),
    )
    .unwrap();
    let matches = Matches(BTreeMap::from([(
        "later".to_owned(),
        "20240802_010203".to_owned(),
    )]));
    let mut writer = Writer::default();
    let mut seams = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut writer,
    };

    let outcome = sync_plaud_preview(
        &PlaudSyncRequest::<SyncPreviewRequest>::new(tree.path().to_path_buf()),
        &mut seams,
    )
    .unwrap();
    assert_eq!(outcome.state.root()["files"]["later"]["status"], "imported");
    assert_eq!(
        outcome.state.root()["files"]["later"]["filename"],
        "later recording"
    );
    assert_eq!(outcome.state.root()["files"]["later"]["filesize"], 12);
}

#[test]
fn new_trash_and_short_recordings_are_skipped() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![
            PlaudFile {
                is_trash: true,
                ..file("trash", 1_725_000_000_000, 60_000)
            },
            file("short", 1_725_000_000_000, 29_999),
        ],
        fail: false,
    };
    let matches = Matches(BTreeMap::new());
    let mut writer = Writer::default();
    let mut seams = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut writer,
    };
    let outcome = sync_plaud_preview(
        &PlaudSyncRequest::<SyncPreviewRequest>::new(tree.path().to_path_buf()),
        &mut seams,
    )
    .unwrap();
    assert_eq!(
        outcome.state.root()["files"]["trash"]["skip_reason"],
        "trashed"
    );
    assert_eq!(
        outcome.state.root()["files"]["short"]["skip_reason"],
        "too_short"
    );
}

#[test]
fn save_orders_newest_first_and_checkpoints_each_success_before_the_next_item() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![
            file("old", 1_725_000_000_000, 60_000),
            file("new", 1_726_000_000_000, 60_000),
        ],
        fail: false,
    };
    let matches = Matches(BTreeMap::new());
    let mut writer = Writer::default();
    let mut downloader = Downloader::default();
    let mut pipeline = ImportedPipeline::default();
    let preview = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut writer,
    };
    let mut seams = PlaudSaveSeams {
        preview,
        download: &mut downloader,
        pipeline: &mut pipeline,
    };

    let outcome = sync_plaud_save(
        &PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf()),
        &mut seams,
    )
    .unwrap();
    assert_eq!(outcome.downloaded, 2);
    assert_eq!(downloader.downloads, ["new", "old"]);
    assert!(
        pipeline
            .calls
            .borrow()
            .iter()
            .all(|(timestamp, auto)| timestamp.is_some() && *auto == PipelineAuto::Enabled)
    );
    assert_eq!(writer.checkpoints.len(), 3);
    assert_eq!(
        writer.checkpoints[0].root()["files"]["new"]["status"],
        "imported"
    );
    assert_eq!(
        writer.checkpoints[1].root()["files"]["old"]["status"],
        "imported"
    );
}

#[test]
fn closed_transport_failures_cannot_persist_or_render_url_or_token_text() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![file("failed", 1_725_000_000_000, 60_000)],
        fail: false,
    };
    let matches = Matches(BTreeMap::new());
    let mut writer = Writer::default();
    let mut downloader = Downloader {
        downloads: Vec::new(),
        fail_download_for: Some("failed"),
    };
    let mut pipeline = PanicPipeline;
    let preview = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut writer,
    };
    let mut seams = PlaudSaveSeams {
        preview,
        download: &mut downloader,
        pipeline: &mut pipeline,
    };
    let outcome = sync_plaud_save(
        &PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf()),
        &mut seams,
    )
    .unwrap();
    let rendered = format!("{outcome:?}");
    assert_eq!(
        outcome.state.root()["files"]["failed"]["last_error"],
        "download failed"
    );
    assert!(!rendered.contains("credential-not-for-state"));
    assert!(!rendered.contains("signature=secret"));
    assert!(
        !format!("{}", PlaudSyncError::Operation(PlaudFailureKind::Download))
            .contains("signature=secret")
    );
}

fn file(id: &str, start_time: i64, duration: i64) -> PlaudFile {
    PlaudFile {
        id: id.to_owned(),
        filename: format!("{id} recording"),
        fullname: format!("{id}.opus"),
        filesize: 12,
        start_time,
        duration,
        is_trash: false,
    }
}
