// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use solstone_core_import::{
    AudioSyncOptions, AutoTimestamp, FileSyncBackend, SyncActionSeams, load_sync_state, sync_audio,
};

#[test]
fn real_m4a_fixtures_are_catalogued_with_container_duration() {
    let temporary = tempfile::tempdir().unwrap();
    let source = temporary.path().join("audio");
    fs::create_dir(&source).unwrap();
    for name in ["aac_multi_track.m4a", "aac_single_track.m4a"] {
        fs::copy(repository_audio_fixture(name), source.join(name)).unwrap();
    }
    fs::write(source.join("corrupt.m4a"), b"not an mp4").unwrap();
    let mut seams = SyncActionSeams {
        per_item_action: |_: solstone_core_import::SyncActionRequest<'_>| Ok(()),
    };
    sync_audio(
        &AudioSyncOptions {
            journal: temporary.path(),
            save: false,
            source_path: Some(&source),
            force: false,
            auto: AutoTimestamp::Absent,
        },
        &mut seams,
    )
    .unwrap();

    let state = load_sync_state(temporary.path(), FileSyncBackend::Audio)
        .unwrap()
        .unwrap();
    for name in ["aac_multi_track.m4a", "aac_single_track.m4a"] {
        let entry = &state.files[name];
        assert_eq!(entry["status"], "skipped");
        let parsed = entry["duration"].as_f64().unwrap();
        assert!(parsed > 0.0, "{name} duration was {parsed}");
        if let Some(oracle) = ffprobe_duration(&source.join(name)) {
            assert!(
                (parsed - oracle).abs() < 0.01,
                "{name}: parser={parsed}, ffprobe={oracle}"
            );
        } else {
            eprintln!("ffprobe unavailable; skipped external duration oracle for {name}");
        }
    }
    assert_eq!(state.files["corrupt.m4a"]["status"], "unreadable");
}

fn repository_audio_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../../tests/fixtures/audio")
        .join(name)
}

fn ffprobe_duration(path: &Path) -> Option<f64> {
    let output = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration",
            "-of",
            "default=noprint_wrappers=1:nokey=1",
        ])
        .arg(path)
        .output()
        .ok()?;
    if !output.status.success() {
        return None;
    }
    std::str::from_utf8(&output.stdout)
        .ok()?
        .trim()
        .parse()
        .ok()
}
