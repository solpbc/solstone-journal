// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use chrono::{DateTime, Duration, Local, TimeZone};
use solstone_core_import::contract::{SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::sync_plaud::{
    FilesystemPlaudStateWriter, ImportPipeline, PipelineAuto, PipelineImportRequest,
    PipelineOutcome, PlaudCatalogue, PlaudCredential, PlaudDownload, PlaudFailureKind, PlaudFile,
    PlaudManifestLookup, PlaudPreviewSeams, PlaudSaveSeams, PlaudStateWriter, PlaudSyncError,
    PlaudSyncRequest, SyncClock, sync_plaud_preview, sync_plaud_save,
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
    fn matching_imports(
        &self,
        _: &[PlaudFile],
    ) -> Result<BTreeMap<String, String>, PlaudFailureKind> {
        Ok(self.0.clone())
    }
}

struct FailingMatches;
impl PlaudManifestLookup for FailingMatches {
    fn matching_imports(
        &self,
        _: &[PlaudFile],
    ) -> Result<BTreeMap<String, String>, PlaudFailureKind> {
        Err(PlaudFailureKind::Manifest)
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

struct EventWriter {
    events: Rc<RefCell<Vec<String>>>,
}
impl PlaudStateWriter for EventWriter {
    fn checkpoint(&mut self, _: &Path, _: &SyncState) -> Result<(), String> {
        self.events.borrow_mut().push("checkpoint".to_owned());
        Ok(())
    }
}

struct PanicPipeline;
impl ImportPipeline for PanicPipeline {
    fn import_one(&mut self, _: PipelineImportRequest<'_>) -> Result<PipelineOutcome, String> {
        panic!("preview has no pipeline authority")
    }
}

struct CheckpointPipeline {
    events: Rc<RefCell<Vec<String>>>,
    calls: Cell<u8>,
}
impl ImportPipeline for CheckpointPipeline {
    fn import_one(&mut self, _: PipelineImportRequest<'_>) -> Result<PipelineOutcome, String> {
        let call = self.calls.get();
        self.calls.set(call + 1);
        self.events.borrow_mut().push(format!("pipeline:{call}"));
        if call == 0 {
            Ok(PipelineOutcome::Imported)
        } else {
            Err("later pipeline failure".to_owned())
        }
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
        files: vec![file("matched", 1_725_000_000_000.5, 60_000.25)],
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
    assert_eq!(entry["start_time"], 1_725_000_000_000.5);
    assert_eq!(entry["duration"], 60_000.25);
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
        files: vec![file("later", 1_725_000_000_000.0, 60_000.0)],
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
                ..file("trash", 1_725_000_000_000.0, 60_000.0)
            },
            file("short", 1_725_000_000_000.0, 29_999.0),
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
            file("old", 1_725_000_000_000.0, 60_000.0),
            file("new", 1_726_000_000_000.0, 60_000.0),
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
fn save_preserves_catalogue_order_for_equal_start_times() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![
            file("first", 1_725_000_000_000.0, 60_000.0),
            file("second", 1_725_000_000_000.0, 60_000.0),
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

    sync_plaud_save(
        &PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf()),
        &mut seams,
    )
    .unwrap();
    assert_eq!(downloader.downloads, ["first", "second"]);
}

#[test]
fn save_keeps_past_and_near_future_timestamps_without_fallback() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let fixed_now = DateTime::parse_from_rfc3339("2026-08-11T12:00:00+00:00").unwrap();
    let past = fixed_now
        .checked_sub_signed(Duration::seconds(1))
        .unwrap()
        .timestamp();
    let near_future = fixed_now
        .checked_add_signed(Duration::seconds(172_799))
        .unwrap()
        .timestamp();
    let mut catalogue = Catalogue {
        files: vec![
            file("past", past as f64, 60_000.0),
            file("near", near_future as f64, 60_000.0),
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

    for (id, timestamp) in [("past", past), ("near", near_future)] {
        let entry = &outcome.state.root()["files"][id];
        assert_eq!(entry["import_timestamp"], device_timestamp(timestamp));
        assert!(entry.get("import_timestamp_fallback").is_none());
    }
}

#[test]
fn save_trusts_timestamp_exactly_48_hours_ahead() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let fixed_now = DateTime::parse_from_rfc3339("2026-08-11T12:00:00+00:00").unwrap();
    let boundary = fixed_now.checked_add_signed(Duration::hours(48)).unwrap();
    assert_eq!(boundary.timestamp(), 1_786_622_400);
    let mut catalogue = Catalogue {
        files: vec![file("boundary", boundary.timestamp() as f64, 60_000.0)],
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
    let entry = &outcome.state.root()["files"]["boundary"];
    assert_eq!(
        entry["import_timestamp"],
        device_timestamp(boundary.timestamp())
    );
    assert!(entry.get("import_timestamp_fallback").is_none());
}

#[test]
fn save_falls_back_for_timestamp_more_than_48_hours_ahead() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let fixed_now = DateTime::parse_from_rfc3339("2026-08-11T12:00:00+00:00").unwrap();
    let future = fixed_now
        .checked_add_signed(Duration::hours(48) + Duration::seconds(1))
        .unwrap();
    let mut catalogue = Catalogue {
        files: vec![file("future", future.timestamp() as f64, 60_000.0)],
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
    let entry = &outcome.state.root()["files"]["future"];
    assert_eq!(entry["import_timestamp"], "20260811_120000");
    assert_eq!(entry["import_timestamp_fallback"], true);
    assert_eq!(
        writer.checkpoints.last().unwrap().root()["files"]["future"]["import_timestamp_fallback"],
        true
    );
}

#[test]
fn save_disambiguates_same_run_fallback_timestamps() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let fixed_now = DateTime::parse_from_rfc3339("2026-08-11T12:00:00+00:00").unwrap();
    let first_future = fixed_now
        .checked_add_signed(Duration::hours(49))
        .unwrap()
        .timestamp();
    let second_future = fixed_now
        .checked_add_signed(Duration::hours(50))
        .unwrap()
        .timestamp();
    let mut catalogue = Catalogue {
        files: vec![
            file("first", first_future as f64, 60_000.0),
            file("second", second_future as f64, 60_000.0),
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
    let mut timestamps = vec![
        outcome.state.root()["files"]["first"]["import_timestamp"]
            .as_str()
            .unwrap()
            .to_owned(),
        outcome.state.root()["files"]["second"]["import_timestamp"]
            .as_str()
            .unwrap()
            .to_owned(),
    ];
    timestamps.sort();
    assert_eq!(timestamps, ["20260811_120000", "20260811_120001"]);
    for id in ["first", "second"] {
        assert_eq!(
            outcome.state.root()["files"][id]["import_timestamp_fallback"],
            true
        );
    }
    assert_eq!(pipeline.calls.borrow().len(), 2);
}

#[test]
fn successful_plaud_item_is_checkpointed_before_a_later_failure() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![
            file("later", 1_725_000_000_000.0, 60_000.0),
            file("first", 1_726_000_000_000.0, 60_000.0),
        ],
        fail: false,
    };
    let matches = Matches(BTreeMap::new());
    let events = Rc::new(RefCell::new(Vec::new()));
    let mut writer = EventWriter {
        events: Rc::clone(&events),
    };
    let mut downloader = Downloader::default();
    let mut pipeline = CheckpointPipeline {
        events: Rc::clone(&events),
        calls: Cell::new(0),
    };
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
    assert_eq!(outcome.downloaded, 1);
    assert_eq!(outcome.errors, ["later recording: import failed"]);
    assert_eq!(outcome.state.root()["files"]["first"]["status"], "imported");
    assert_eq!(
        outcome.state.root()["files"]["later"]["status"],
        "available"
    );
    assert_eq!(
        outcome.state.root()["files"]["later"]["last_error"],
        "import failed"
    );
    assert_eq!(
        events.borrow().as_slice(),
        ["pipeline:0", "checkpoint", "pipeline:1", "checkpoint"]
    );
}

#[test]
fn failed_available_recording_is_retried_on_the_next_save() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let matches = Matches(BTreeMap::new());
    let request = PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf());

    let mut first_catalogue = Catalogue {
        files: vec![file("retry", 1_725_000_000_000.0, 60_000.0)],
        fail: false,
    };
    let mut first_writer = Writer::default();
    let mut failing_download = Downloader {
        downloads: Vec::new(),
        fail_download_for: Some("retry"),
    };
    let mut first_pipeline = PanicPipeline;
    let preview = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut first_catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut first_writer,
    };
    let mut first_seams = PlaudSaveSeams {
        preview,
        download: &mut failing_download,
        pipeline: &mut first_pipeline,
    };
    let first = sync_plaud_save(&request, &mut first_seams).unwrap();
    assert_eq!(first.state.root()["files"]["retry"]["status"], "available");
    assert_eq!(first.errors, ["retry recording: download failed"]);
    solstone_core_import::write_sync_state(tree.path(), &first.state).unwrap();

    let mut second_catalogue = Catalogue {
        files: vec![file("retry", 1_725_000_000_000.0, 60_000.0)],
        fail: false,
    };
    let mut second_writer = Writer::default();
    let mut working_download = Downloader::default();
    let mut working_pipeline = ImportedPipeline::default();
    let preview = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut second_catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut second_writer,
    };
    let mut second_seams = PlaudSaveSeams {
        preview,
        download: &mut working_download,
        pipeline: &mut working_pipeline,
    };
    let second = sync_plaud_save(&request, &mut second_seams).unwrap();
    assert_eq!(second.downloaded, 1);
    assert!(second.errors.is_empty());
    assert_eq!(second.state.root()["files"]["retry"]["status"], "imported");
    assert_eq!(working_download.downloads, ["retry"]);
}

#[test]
fn manifest_failures_render_only_the_closed_failure_kind() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![file("matched", 1_725_000_000_000.0, 60_000.0)],
        fail: false,
    };
    let matches = FailingMatches;
    let mut writer = Writer::default();
    let mut seams = PlaudPreviewSeams {
        credential: &credential,
        catalogue: &mut catalogue,
        manifests: &matches,
        clock: &clock,
        state_writer: &mut writer,
    };

    let error = sync_plaud_preview(
        &PlaudSyncRequest::<SyncPreviewRequest>::new(tree.path().to_path_buf()),
        &mut seams,
    )
    .unwrap_err();
    assert_eq!(error, PlaudSyncError::Operation(PlaudFailureKind::Manifest));
    assert_eq!(format!("{error}"), "Plaud import matching failed");
}

#[test]
fn closed_transport_failures_cannot_persist_or_render_url_or_token_text() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![file("failed", 1_725_000_000_000.0, 60_000.0)],
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

struct SentinelCredential;
impl PlaudCredential for SentinelCredential {
    fn access_token(&self) -> Option<&str> {
        Some(SENTINEL)
    }
}

const SENTINEL: &str = "PLAUD-ACCESS-TOKEN-SENTINEL-DO-NOT-PERSIST";

/// Every regular file under `root`, so the assertion below covers the whole tree rather than
/// the one state file a reader would think to check.
fn every_file_under(root: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in std::fs::read_dir(&directory).expect("journal tree is readable") {
            let path = entry.expect("directory entry is readable").path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found
}

#[test]
fn credential_reaches_no_file_in_the_owner_s_journal_tree() {
    let tree = TempDir::new().unwrap();
    let credential = SentinelCredential;
    let clock = Clock;
    let mut catalogue = Catalogue {
        files: vec![file("matched", 1_725_000_000_000.5, 60_000.25)],
        fail: false,
    };
    let matches = Matches(BTreeMap::from([(
        "matched".to_owned(),
        "20240801_010203".to_owned(),
    )]));
    // The real writer, not the recording fake: this test is about bytes on disk, and a fake
    // state writer would make it pass without the journal ever being written to.
    let mut writer = FilesystemPlaudStateWriter;
    let mut downloader = Downloader::default();
    let mut pipeline = ImportedPipeline::default();
    let mut seams = PlaudSaveSeams {
        preview: PlaudPreviewSeams {
            credential: &credential,
            catalogue: &mut catalogue,
            manifests: &matches,
            clock: &clock,
            state_writer: &mut writer,
        },
        download: &mut downloader,
        pipeline: &mut pipeline,
    };
    let request = PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf());

    let outcome = sync_plaud_save(&request, &mut seams).unwrap();

    let written = every_file_under(tree.path());

    // Positive control, and it is the point of the test rather than decoration. A tree walk
    // that visits nothing satisfies every "does not contain" assertion below, so prove the
    // instrument can both see files and find the sentinel before trusting its absence.
    assert!(
        !written.is_empty(),
        "walk found no files, so the sentinel checks below would pass vacuously"
    );
    let planted = tree.path().join("planted-control");
    std::fs::write(&planted, format!("leading {SENTINEL} trailing")).unwrap();
    let control = every_file_under(tree.path())
        .iter()
        .filter(|path| {
            String::from_utf8_lossy(&std::fs::read(path).unwrap_or_default()).contains(SENTINEL)
        })
        .count();
    assert_eq!(
        control, 1,
        "walk cannot detect a planted sentinel, so it cannot testify to its absence"
    );
    std::fs::remove_file(&planted).unwrap();

    for path in every_file_under(tree.path()) {
        let bytes = std::fs::read(&path).expect("written file is readable");
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL),
            "credential reached the owner's journal at {}",
            path.display()
        );
    }

    // The state file specifically must exist, or the walk proved nothing about the write path.
    assert!(
        written
            .iter()
            .any(|path| path == &state_path(tree.path(), BackendName::Plaud)),
        "sync did not write its state file, so no persistence path was exercised"
    );
    assert!(!format!("{outcome:?}").contains(SENTINEL));
    assert!(!format!("{:?}", outcome.errors).contains(SENTINEL));
}

fn file(id: &str, start_time: f64, duration: f64) -> PlaudFile {
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

fn device_timestamp(seconds: i64) -> String {
    Local
        .timestamp_opt(seconds, 0)
        .single()
        .unwrap()
        .format("%Y%m%d_%H%M%S")
        .to_string()
}
