// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::path::{Path, PathBuf};

use solstone_core_import::SyncState;
use solstone_core_import::contract::{AudioAuto, SyncPreviewRequest, SyncSaveRequest};
use solstone_core_import::sync_audio::{
    AudioCandidate, AudioPreviewSeams, AudioProbe, AudioSaveSeams, AudioStateWriter,
    AudioSyncError, AudioSyncRequest, DirectoryScanner, ManifestLookup, sync_audio_preview,
    sync_audio_save,
};
use solstone_core_import::sync_plaud::{
    ImportPipeline, PipelineAuto, PipelineImportRequest, PipelineOutcome, SyncClock,
};
use tempfile::TempDir;

struct Scanner;
impl DirectoryScanner for Scanner {
    fn audio_candidates(&self, _: &Path) -> Result<Vec<AudioCandidate>, String> {
        Ok(vec![
            candidate("already.wav", "already"),
            candidate("first.wav", "first"),
            candidate("failed.wav", "failed"),
        ])
    }
}

struct OneScanner;
impl DirectoryScanner for OneScanner {
    fn audio_candidates(&self, _: &Path) -> Result<Vec<AudioCandidate>, String> {
        Ok(vec![candidate("current.wav", "current")])
    }
}

fn candidate(name: &str, hash: &str) -> AudioCandidate {
    AudioCandidate {
        relative_path: name.to_owned(),
        source: PathBuf::from(name),
        filename: name.to_owned(),
        filesize: 10,
        source_hash: hash.to_owned(),
    }
}

struct Probe;
impl AudioProbe for Probe {
    fn duration_seconds(&self, _: &Path) -> Result<Option<f64>, String> {
        Ok(Some(60.0))
    }
}

struct FractionalProbe;
impl AudioProbe for FractionalProbe {
    fn duration_seconds(&self, _: &Path) -> Result<Option<f64>, String> {
        Ok(Some(29.5))
    }
}

struct Manifests;
impl ManifestLookup for Manifests {
    fn imported_hash(&self, hash: &str) -> bool {
        hash == "already"
    }
}

struct Clock;
impl SyncClock for Clock {
    fn now(&self) -> String {
        "2026-08-11T12:00:00+00:00".to_owned()
    }
}

#[derive(Default)]
struct Writer {
    checkpoints: Vec<SyncState>,
}
impl AudioStateWriter for Writer {
    fn checkpoint(&mut self, _: &Path, state: &SyncState) -> Result<(), String> {
        self.checkpoints.push(state.clone());
        Ok(())
    }
}

struct FailingPipeline(Cell<u32>);
impl ImportPipeline for FailingPipeline {
    fn import_one(&mut self, _: PipelineImportRequest<'_>) -> Result<PipelineOutcome, String> {
        if self.0.get() == 0 {
            self.0.set(1);
            Ok(PipelineOutcome::Imported)
        } else {
            Err("pipeline failed".to_owned())
        }
    }
}

#[test]
fn preview_has_no_pipeline_authority() {
    let tree = TempDir::new().unwrap();
    let scanner = Scanner;
    let probe = Probe;
    let manifests = Manifests;
    let clock = Clock;
    let mut writer = Writer::default();
    let mut seams = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let request = AudioSyncRequest::<SyncPreviewRequest>::new(
        tree.path().to_path_buf(),
        PathBuf::from("audio"),
        false,
        AudioAuto::Enabled,
    );
    let outcome = sync_audio_preview(&request, &mut seams).unwrap();
    assert_eq!(outcome.downloaded, 0);
}

#[test]
fn failure_keeps_imported_items_and_checkpoints_failed_item_state() {
    let tree = TempDir::new().unwrap();
    let scanner = Scanner;
    let probe = Probe;
    let manifests = Manifests;
    let clock = Clock;
    let mut writer = Writer::default();
    let mut pipeline = FailingPipeline(Cell::new(0));
    let preview = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let mut seams = AudioSaveSeams {
        preview,
        pipeline: &mut pipeline,
    };
    let request = AudioSyncRequest::<SyncSaveRequest>::new(
        tree.path().to_path_buf(),
        PathBuf::from("audio"),
        false,
        AudioAuto::Enabled,
    );
    let outcome = sync_audio_save(&request, &mut seams).unwrap();
    assert_eq!(
        outcome.state.root()["files"]["already.wav"]["status"],
        "imported"
    );
    assert_eq!(
        outcome.state.root()["files"]["first.wav"]["status"],
        "imported"
    );
    assert_eq!(
        outcome.state.root()["files"]["failed.wav"]["status"],
        "available"
    );
    assert_eq!(
        outcome.state.root()["files"]["failed.wav"]["last_error"],
        "pipeline failed"
    );
    assert_eq!(outcome.errors, vec!["failed.wav: pipeline failed"]);
    assert_eq!(outcome.items[1].relative_path, "failed.wav");
    assert!(outcome.items[0].checkpointed);
    assert!(outcome.items[1].checkpointed);
    assert_eq!(
        writer.checkpoints[0].root()["files"]["first.wav"]["status"],
        "imported"
    );
    assert!(
        writer.checkpoints.iter().any(|state| {
            state.root()["files"]["failed.wav"]["last_error"] == "pipeline failed"
        })
    );
}

#[test]
fn missing_source_is_a_named_refusal() {
    let tree = TempDir::new().unwrap();
    let scanner = Scanner;
    let probe = Probe;
    let manifests = Manifests;
    let clock = Clock;
    let mut writer = Writer::default();
    let mut seams = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let request = AudioSyncRequest::<SyncPreviewRequest>::new(
        tree.path().to_path_buf(),
        PathBuf::new(),
        false,
        AudioAuto::Enabled,
    );
    assert!(matches!(
        sync_audio_preview(&request, &mut seams),
        Err(AudioSyncError::MissingSource)
    ));
}

#[test]
fn fractional_duration_uses_the_reference_threshold_without_truncation() {
    let tree = TempDir::new().unwrap();
    let scanner = OneScanner;
    let probe = FractionalProbe;
    let manifests = Manifests;
    let clock = Clock;
    let mut writer = Writer::default();
    let mut seams = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let request = AudioSyncRequest::<SyncPreviewRequest>::new(
        tree.path().to_path_buf(),
        PathBuf::from("audio"),
        false,
        AudioAuto::Enabled,
    );

    let outcome = sync_audio_preview(&request, &mut seams).unwrap();
    assert_eq!(
        outcome.state.root()["files"]["current.wav"]["status"],
        "skipped"
    );
    assert_eq!(
        outcome.state.root()["files"]["current.wav"]["duration"],
        29.5
    );
}

#[test]
fn unseen_preexisting_manifest_match_is_removed_like_the_reference() {
    let tree = TempDir::new().unwrap();
    let scanner = OneScanner;
    let probe = Probe;
    let manifests = Manifests;
    let clock = Clock;
    let mut state = SyncState::empty(solstone_core_import::BackendName::Audio);
    state.files_mut().insert(
        "manually-imported.wav".to_owned(),
        serde_json::json!({"status": "available", "hash": "already"}),
    );
    solstone_core_import::write_sync_state(tree.path(), &state).unwrap();
    let mut writer = Writer::default();
    let mut seams = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let request = AudioSyncRequest::<SyncPreviewRequest>::new(
        tree.path().to_path_buf(),
        PathBuf::from("audio"),
        false,
        AudioAuto::Enabled,
    );

    let outcome = sync_audio_preview(&request, &mut seams).unwrap();
    assert_eq!(
        outcome.state.root()["files"]["manually-imported.wav"]["status"],
        "removed"
    );
}

struct AutoRecordingPipeline(Option<String>);
impl ImportPipeline for AutoRecordingPipeline {
    fn import_one(
        &mut self,
        request: PipelineImportRequest<'_>,
    ) -> Result<PipelineOutcome, String> {
        self.0 = Some(match request.auto {
            PipelineAuto::Enabled => "enabled".to_owned(),
            PipelineAuto::Disabled => "disabled".to_owned(),
            PipelineAuto::Value(value) => value.to_owned(),
        });
        Ok(PipelineOutcome::Imported)
    }
}

#[test]
fn string_auto_mode_reaches_the_pipeline_unchanged() {
    let tree = TempDir::new().unwrap();
    let scanner = Scanner;
    let probe = Probe;
    let manifests = Manifests;
    let clock = Clock;
    let mut writer = Writer::default();
    let mut pipeline = AutoRecordingPipeline(None);
    let preview = AudioPreviewSeams {
        scanner: &scanner,
        probe: &probe,
        manifests: &manifests,
        clock: &clock,
        state_writer: &mut writer,
    };
    let mut seams = AudioSaveSeams {
        preview,
        pipeline: &mut pipeline,
    };
    let request = AudioSyncRequest::<SyncSaveRequest>::new(
        tree.path().to_path_buf(),
        PathBuf::from("audio"),
        false,
        AudioAuto::Value("quiet".to_owned()),
    );

    sync_audio_save(&request, &mut seams).unwrap();
    assert_eq!(pipeline.0.as_deref(), Some("quiet"));
}
