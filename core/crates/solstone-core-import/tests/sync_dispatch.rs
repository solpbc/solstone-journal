// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::cell::Cell;
use std::fs;
use std::path::Path;

use solstone_core_import::{
    AudioSyncOptions, AutoTimestamp, FileSyncBackend, ObsidianSyncOptions, PlaudHttp,
    PlaudRemoteFile, PlaudSyncOptions, SyncActionFailure, SyncActionSeams, SyncBackend,
    available_sync_backends, load_sync_state, sync_audio, sync_obsidian, sync_plaud_with_http,
};

struct FakePlaudHttp;

impl PlaudHttp for FakePlaudHttp {
    fn list_files(
        &mut self,
        _token: &str,
    ) -> Result<Vec<PlaudRemoteFile>, solstone_core_import::ImportError> {
        Ok(["first", "second"]
            .into_iter()
            .map(|id| PlaudRemoteFile {
                id: id.to_owned(),
                filename: format!("{id}.mp3"),
                fullname: id.to_owned(),
                filesize: 1,
                start_time: 1,
                duration: 31_000,
                is_trash: false,
            })
            .collect())
    }
}

#[test]
fn backends_are_oracle_order_plus_oura() {
    assert_eq!(
        available_sync_backends()
            .iter()
            .map(|backend| backend.name())
            .collect::<Vec<_>>(),
        ["plaud", "obsidian", "audio", "oura"]
    );
}

#[test]
fn catalog_never_calls_action_and_save_calls_each_available_action() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    let vault = temporary.path().join("vault");
    let audio = temporary.path().join("audio");
    fs::create_dir_all(&journal).unwrap();
    fs::create_dir_all(&vault).unwrap();
    fs::create_dir_all(&audio).unwrap();
    fs::write(vault.join("note.md"), "A real note").unwrap();
    write_m4a(&audio.join("recording.m4a"), 31);

    let calls = Cell::new(0);
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| {
            calls.set(calls.get() + 1);
            Ok(())
        },
    };
    let mut http = FakePlaudHttp;
    sync_plaud_with_http(
        &PlaudSyncOptions {
            journal: &journal,
            save: false,
            access_token: "sentinel",
        },
        &mut http,
        &mut seams,
    )
    .unwrap();
    sync_obsidian(
        &ObsidianSyncOptions {
            journal: &journal,
            save: false,
            source_path: Some(&vault),
            force: false,
        },
        &mut seams,
    )
    .unwrap();
    sync_audio(
        &AudioSyncOptions {
            journal: &journal,
            save: false,
            source_path: Some(&audio),
            force: false,
            auto: AutoTimestamp::Absent,
        },
        &mut seams,
    )
    .unwrap();
    assert_eq!(calls.get(), 0);

    sync_plaud_with_http(
        &PlaudSyncOptions {
            journal: &journal,
            save: true,
            access_token: "sentinel",
        },
        &mut http,
        &mut seams,
    )
    .unwrap();
    sync_obsidian(
        &ObsidianSyncOptions {
            journal: &journal,
            save: true,
            source_path: Some(&vault),
            force: false,
        },
        &mut seams,
    )
    .unwrap();
    sync_audio(
        &AudioSyncOptions {
            journal: &journal,
            save: true,
            source_path: Some(&audio),
            force: false,
            auto: AutoTimestamp::Absent,
        },
        &mut seams,
    )
    .unwrap();
    assert_eq!(calls.get(), 4);
}

#[test]
fn path_and_window_are_routed_only_to_their_supported_backends() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path().join("journal");
    let vault = temporary.path().join("vault");
    let audio = temporary.path().join("audio");
    fs::create_dir_all(&journal).unwrap();
    fs::create_dir_all(&vault).unwrap();
    fs::create_dir_all(&audio).unwrap();
    fs::write(vault.join("only-vault.md"), "vault").unwrap();
    write_m4a(&audio.join("only-audio.m4a"), 31);
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| Ok(()),
    };

    sync_obsidian(
        &ObsidianSyncOptions {
            journal: &journal,
            save: false,
            source_path: Some(&vault),
            force: false,
        },
        &mut seams,
    )
    .unwrap();
    sync_audio(
        &AudioSyncOptions {
            journal: &journal,
            save: false,
            source_path: Some(&audio),
            force: false,
            auto: AutoTimestamp::Absent,
        },
        &mut seams,
    )
    .unwrap();
    assert!(
        load_sync_state(&journal, FileSyncBackend::Obsidian)
            .unwrap()
            .unwrap()
            .files
            .contains_key("only-vault.md")
    );
    assert!(
        load_sync_state(&journal, FileSyncBackend::Audio)
            .unwrap()
            .unwrap()
            .files
            .contains_key("only-audio.m4a")
    );
    assert_eq!(SyncBackend::Oura.name(), "oura");
}

#[test]
fn partial_action_failure_persists_prior_successes_and_reports_each_failure() {
    let temporary = tempfile::tempdir().unwrap();
    let journal = temporary.path();
    let mut http = FakePlaudHttp;
    let mut seams = SyncActionSeams {
        per_item_action: |request: solstone_core_import::SyncActionRequest<'_>| {
            if request.item_key == "second" {
                Err(SyncActionFailure {
                    message: "source unavailable".to_owned(),
                })
            } else {
                Ok(())
            }
        },
    };
    let report = sync_plaud_with_http(
        &PlaudSyncOptions {
            journal,
            save: true,
            access_token: "sentinel",
        },
        &mut http,
        &mut seams,
    )
    .unwrap();

    assert_eq!(report.imported, 1);
    assert_eq!(report.failures.len(), 1);
    assert_eq!(report.failures[0].item, "second.mp3");
    assert_eq!(report.failures[0].reason, "source unavailable");
    let state = load_sync_state(journal, FileSyncBackend::Plaud)
        .unwrap()
        .unwrap();
    assert_eq!(state.files["first"]["status"], "imported");
    assert_eq!(state.files["second"]["status"], "available");
    assert_eq!(state.files["second"]["last_error"], "source unavailable");
}

fn write_m4a(path: &Path, duration_seconds: u32) {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&16_u32.to_be_bytes());
    bytes.extend_from_slice(b"ftyp");
    bytes.extend_from_slice(b"M4A ");
    bytes.extend_from_slice(&0_u32.to_be_bytes());
    let mut mvhd = vec![0];
    mvhd.extend_from_slice(&[0; 3]);
    mvhd.extend_from_slice(&0_u32.to_be_bytes());
    mvhd.extend_from_slice(&0_u32.to_be_bytes());
    mvhd.extend_from_slice(&1_000_u32.to_be_bytes());
    mvhd.extend_from_slice(&(duration_seconds * 1_000).to_be_bytes());
    let size = u32::try_from(mvhd.len() + 8).unwrap();
    let mut moov = Vec::new();
    moov.extend_from_slice(&size.to_be_bytes());
    moov.extend_from_slice(b"mvhd");
    moov.extend_from_slice(&mvhd);
    bytes.extend_from_slice(&u32::try_from(moov.len() + 8).unwrap().to_be_bytes());
    bytes.extend_from_slice(b"moov");
    bytes.extend_from_slice(&moov);
    fs::write(path, bytes).unwrap();
}
