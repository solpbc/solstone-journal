// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity_obligations::observe_obligations;
use solstone_core_journal_io::media::MediaDirectory;
use std::{ffi::OsStr, fs};

#[test]
fn unpublished_recorded_restore_releases_stage_and_intent_on_windows() {
    let temp = tempfile::tempdir().unwrap();
    let mut intent = crate::restore_intent::tests::fixture(false);
    let segment_path = temp.path().join(intent.locator.relative_segment());
    fs::create_dir_all(&segment_path).unwrap();
    let stage_path = segment_path
        .parent()
        .unwrap()
        .join(&intent.stage.stage_name);
    fs::create_dir(&stage_path).unwrap();
    fs::write(
        stage_path.join(&intent.stage.leaf_name),
        b"unpublished copy",
    )
    .unwrap();
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    {
        let relative = intent.locator.relative_segment();
        let parent = MediaDirectory::open(Arc::clone(&root), relative.parent().unwrap()).unwrap();
        let segment = parent
            .open_child(OsStr::new(&intent.locator.target.dir))
            .unwrap()
            .unwrap();
        let stage = parent
            .open_child(OsStr::new(&intent.stage.stage_name))
            .unwrap()
            .unwrap();
        let leaf = stage
            .observe_file(OsStr::new(&intent.stage.leaf_name))
            .unwrap()
            .unwrap();
        intent.destination.root = root.identity().observation_id();
        intent.destination.parent = parent.identity();
        intent.destination.segment = segment.identity();
        intent.stage.parent = parent.identity();
        intent.stage.directory = stage.identity();
        intent.stage.leaf = leaf.identity;
        intent.stage.boot_id = None;
    }
    let intents = temp.path().join("health/offload/restore-intents");
    fs::create_dir_all(&intents).unwrap();
    let path = intents.join(format!("{}.json", intent.event.operation_id));
    fs::write(&path, intent.encode().unwrap()).unwrap();
    let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    let recovered = recover_abandoned_restores(&activity).unwrap();
    assert_eq!(recovered.len(), 1, "{recovered:?}");
    assert_eq!(recovered[0].outcome, Ok(AbandonedRestoreRelease::Undone));
    assert!(!stage_path.exists());
    assert!(!path.exists());
    assert!(observe_obligations(&root).unwrap().entries().is_empty());
}
