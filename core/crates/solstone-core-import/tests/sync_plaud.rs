// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::fs;
use std::path::Path;

use solstone_core_import::contract::{SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::sync_plaud::{
    ImportPipeline, PipelineOutcome, PlaudCredential, PlaudFile, PlaudHttp, PlaudHttpError,
    PlaudSyncRequest, PlaudSyncSeams, SyncClock, sync_plaud_preview, sync_plaud_save,
};
use solstone_core_import::{BackendName, state_path};
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

struct Http {
    files: Vec<PlaudFile>,
    downloads: Cell<u32>,
    panic_on_download: bool,
    fail_download_for: Option<&'static str>,
}
impl PlaudHttp for Http {
    fn list_files(&mut self, _: &str) -> Result<Vec<PlaudFile>, PlaudHttpError> {
        Ok(self.files.clone())
    }
    fn temporary_url(&mut self, _: &str, file_id: &str) -> Result<String, PlaudHttpError> {
        Ok(format!("https://example.invalid/{file_id}"))
    }
    fn download(&mut self, url: &str, _: &Path) -> Result<(), PlaudHttpError> {
        assert!(!self.panic_on_download, "preview must not download");
        if self
            .fail_download_for
            .is_some_and(|file_id| url.ends_with(file_id))
        {
            return Err(PlaudHttpError {
                message: "download failed".to_owned(),
            });
        }
        self.downloads.set(self.downloads.get() + 1);
        Ok(())
    }
}

struct PanicPipeline;
impl ImportPipeline for PanicPipeline {
    fn import_one(&mut self, _: &Path, _: &str, _: bool) -> Result<PipelineOutcome, String> {
        panic!("preview must not call pipeline")
    }
}

struct ImportedPipeline;
impl ImportPipeline for ImportedPipeline {
    fn import_one(&mut self, _: &Path, _: &str, _: bool) -> Result<PipelineOutcome, String> {
        Ok(PipelineOutcome::Imported)
    }
}

#[test]
fn preview_never_downloads_or_invokes_pipeline_and_never_persists_credential() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut http = Http {
        files: vec![file("file-1")],
        downloads: Cell::new(0),
        panic_on_download: true,
        fail_download_for: None,
    };
    let mut pipeline = PanicPipeline;
    let mut seams = PlaudSyncSeams {
        credential: &credential,
        http: &mut http,
        clock: &clock,
        pipeline: &mut pipeline,
    };
    let request = PlaudSyncRequest::<SyncPreviewRequest>::new(tree.path().to_path_buf());

    let outcome = sync_plaud_preview(&request, &mut seams).unwrap();
    assert_eq!(outcome.downloaded, 0);
    assert_eq!(http.downloads.get(), 0);
    let state = fs::read_to_string(state_path(tree.path(), BackendName::Plaud)).unwrap();
    assert!(!state.contains("credential-not-for-state"));
}

#[test]
fn save_downloads_and_imports_once() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut http = Http {
        files: vec![file("file-1")],
        downloads: Cell::new(0),
        panic_on_download: false,
        fail_download_for: None,
    };
    let mut pipeline = ImportedPipeline;
    let mut seams = PlaudSyncSeams {
        credential: &credential,
        http: &mut http,
        clock: &clock,
        pipeline: &mut pipeline,
    };
    let request = PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf());

    let outcome = sync_plaud_save(&request, &mut seams).unwrap();
    assert_eq!(outcome.downloaded, 1);
    assert_eq!(http.downloads.get(), 1);
    assert_eq!(
        outcome.state.root()["files"]["file-1"]["status"],
        "imported"
    );
}

#[test]
fn mid_sync_failure_preserves_an_already_imported_plaud_item() {
    let tree = TempDir::new().unwrap();
    let credential = Credential;
    let clock = Clock;
    let mut http = Http {
        files: vec![file("file-1"), file("file-2")],
        downloads: Cell::new(0),
        panic_on_download: false,
        fail_download_for: Some("file-2"),
    };
    let mut pipeline = ImportedPipeline;
    let mut seams = PlaudSyncSeams {
        credential: &credential,
        http: &mut http,
        clock: &clock,
        pipeline: &mut pipeline,
    };
    let request = PlaudSyncRequest::<SyncSaveRequest>::new(tree.path().to_path_buf());

    let outcome = sync_plaud_save(&request, &mut seams).unwrap();
    assert_eq!(
        outcome.state.root()["files"]["file-1"]["status"],
        "imported"
    );
    assert_eq!(
        outcome.state.root()["files"]["file-2"]["status"],
        "available"
    );
    assert_eq!(
        outcome.state.root()["files"]["file-2"]["last_error"],
        "download failed"
    );
    assert_eq!(outcome.errors, vec!["file-2.opus: download failed"]);
}

fn file(id: &str) -> PlaudFile {
    PlaudFile {
        id: id.to_owned(),
        filename: format!("{id}.opus"),
        filesize: 12,
        start_time: "2026-08-11T10:00:00+00:00".to_owned(),
        trashed: false,
    }
}
