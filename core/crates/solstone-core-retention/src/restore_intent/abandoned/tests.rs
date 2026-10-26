// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity_obligations::observe_obligations;
use solstone_core_journal_io::media::{MediaIdentity, digest_reader};

use super::*;
use crate::door::remove_segments;
use crate::restore_intent::tests::fixture;
use crate::{RemovalReason, Target};

const OPERATION: &str = "ffffffffffffffffffffffffffffffff";
const RESTORED: &[u8] = b"restored audio bytes";

fn identity(path: &Path) -> MediaIdentity {
    let metadata = fs::symlink_metadata(path).unwrap();
    MediaIdentity::Unix {
        device: metadata.dev(),
        inode: metadata.ino(),
    }
}

struct Interrupted {
    _temp: tempfile::TempDir,
    journal: PathBuf,
    root: Arc<JournalRoot>,
    segment: PathBuf,
    stage: PathBuf,
    intent: PathBuf,
    target: Target,
}

/// A crash-like drop after staging: the stage holds the unpublished copy and
/// the intent is durable, but nothing was published, recorded or marked.
fn interrupted() -> Interrupted {
    interrupted_with("payload")
}

fn interrupted_with(leaf: &str) -> Interrupted {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path().to_owned();
    let mut intent = fixture(false);
    let segment = journal.join(intent.locator.relative_segment());
    fs::create_dir_all(&segment).unwrap();
    fs::write(segment.join("other.jsonl"), b"{}\n").unwrap();
    let parent = segment.parent().unwrap().to_owned();
    let stage = parent.join(&intent.stage.stage_name);
    fs::create_dir(&stage).unwrap();
    fs::write(stage.join(leaf), RESTORED).unwrap();
    let root = Arc::new(JournalRoot::open(&journal).unwrap());
    let digest = digest_reader(&mut &RESTORED[..]).unwrap();
    intent.witness.file.bytes = digest.size;
    intent.witness.file.sha256 = digest.sha256;
    intent.destination.root = root.identity().observation_id();
    intent.destination.parent = identity(&parent);
    intent.destination.segment = identity(&segment);
    intent.stage.parent = identity(&parent);
    intent.stage.directory = identity(&stage);
    intent.stage.leaf = identity(&stage.join(leaf));
    intent.stage.leaf_name = leaf.into();
    let intents = journal.join("health/offload/restore-intents");
    fs::create_dir_all(&intents).unwrap();
    let path = intents.join(format!("{OPERATION}.json"));
    fs::write(&path, intent.encode().unwrap()).unwrap();
    let target = intent.locator.target.clone();
    Interrupted {
        _temp: temp,
        journal,
        root,
        segment,
        stage,
        intent: path,
        target,
    }
}

fn delete(fixture: &Interrupted) -> crate::Outcome {
    remove_segments(
        &fixture.journal,
        std::slice::from_ref(&fixture.target),
        "2026-10-11T00:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    )
}

fn recover(fixture: &Interrupted) -> Vec<AbandonedRestore> {
    let activity = ActivityLease::try_acquire(Arc::clone(&fixture.root)).unwrap();
    recover_abandoned_restores(&activity).expect("local recovery")
}

#[test]
fn unpublished_restore_is_undone_and_owner_deletion_then_proceeds() {
    // Earlier intents recorded the shared leaf; current ones the dedicated one.
    for leaf in ["payload", super::super::RESTORE_DOWNLOAD_LEAF] {
        let interrupted = interrupted_with(leaf);
        assert_eq!(
            observe_obligations(&interrupted.root)
                .unwrap()
                .entries()
                .len(),
            2
        );
        // Owner deletion itself recovers the interrupted restore under its lease.
        let removed = delete(&interrupted);
        assert!(removed.targets[0].not_removed.is_empty(), "{removed:?}");
        assert!(!removed.targets[0].removed.is_empty());
        assert!(!interrupted.stage.exists());
        assert!(!interrupted.intent.exists());
        assert!(
            observe_obligations(&interrupted.root)
                .unwrap()
                .entries()
                .is_empty()
        );
    }
}

#[test]
fn exactly_published_restore_is_released_and_deletion_then_proceeds() {
    for linked in [true, false] {
        let interrupted = interrupted();
        let published = interrupted.segment.join("a.flac");
        if linked {
            // Crash right after the create-only link, before its outcome persisted.
            fs::hard_link(interrupted.stage.join("payload"), &published).unwrap();
        } else {
            fs::write(&published, RESTORED).unwrap();
        }
        let outcome = recover(&interrupted);
        assert_eq!(outcome.len(), 1);
        assert_eq!(outcome[0].operation_id.as_deref(), Some(OPERATION));
        assert_eq!(
            outcome[0].outcome,
            Ok(AbandonedRestoreRelease::PublishedExact)
        );
        assert_eq!(fs::read(&published).unwrap(), RESTORED);
        assert!(!interrupted.stage.exists());
        assert!(!interrupted.intent.exists());
        // Nothing was recorded: the ledger and marks were never written.
        assert!(
            !interrupted
                .journal
                .join("health/retention-marks.json")
                .exists()
        );
        let removed = delete(&interrupted);
        assert!(removed.targets[0].not_removed.is_empty(), "{removed:?}");
    }
}

#[test]
fn mismatched_published_file_is_held_and_never_touched() {
    let interrupted = interrupted();
    let published = interrupted.segment.join("a.flac");
    fs::write(&published, b"other content").unwrap();
    let outcome = recover(&interrupted);
    assert_eq!(
        outcome[0].outcome,
        Err(AbandonedRestoreHold::PublishedContentMismatch)
    );
    let held = delete(&interrupted);
    assert!(held.targets[0].removed.is_empty());
    assert_eq!(fs::read(&published).unwrap(), b"other content");
    assert_eq!(
        fs::read(interrupted.stage.join("payload")).unwrap(),
        RESTORED
    );
    assert!(interrupted.intent.exists());
}

#[test]
fn a_stage_holding_anything_else_is_held() {
    let interrupted = interrupted();
    fs::write(interrupted.stage.join("unexpected"), b"keep").unwrap();
    let outcome = recover(&interrupted);
    assert_eq!(outcome[0].outcome, Err(AbandonedRestoreHold::StageChanged));
    assert_eq!(
        fs::read(interrupted.stage.join("payload")).unwrap(),
        RESTORED
    );
    assert_eq!(
        fs::read(interrupted.stage.join("unexpected")).unwrap(),
        b"keep"
    );
    assert!(interrupted.intent.exists());
    // A replaced copy is not the restore's own and is never removed either.
    let interrupted = self::interrupted();
    let original = interrupted.journal.join("original-payload");
    fs::rename(interrupted.stage.join("payload"), &original).unwrap();
    fs::write(interrupted.stage.join("payload"), RESTORED).unwrap();
    let outcome = recover(&interrupted);
    assert_eq!(outcome[0].outcome, Err(AbandonedRestoreHold::StageChanged));
    assert!(interrupted.stage.join("payload").exists());
    assert!(interrupted.intent.exists());
}

/// A restore that ended mid-download, before writing its intent.
fn unrecorded(journal: &Path, relative: &str, leaves: &[&str]) -> PathBuf {
    let stage = journal.join(relative);
    fs::create_dir_all(&stage).unwrap();
    for leaf in leaves {
        fs::write(stage.join(leaf), RESTORED).unwrap();
    }
    stage
}

#[test]
fn download_stage_without_intent_is_recovered_and_deletion_then_proceeds() {
    let temp = tempfile::tempdir().unwrap();
    let journal = temp.path();
    let segment = journal.join("chronicle/20261009/camera/120000_60_suffix");
    fs::create_dir_all(&segment).unwrap();
    fs::write(segment.join("other.jsonl"), b"{}\n").unwrap();
    let named = unrecorded(
        journal,
        "chronicle/20261009/camera/.tmp-offload-0123456789abcdef0123456789abcdef",
        &[super::super::RESTORE_DOWNLOAD_LEAF],
    );
    let direct = unrecorded(
        journal,
        "chronicle/20261009/.tmp-offload-fedcba9876543210fedcba9876543210",
        &[super::super::RESTORE_DOWNLOAD_LEAF],
    );
    let root = Arc::new(JournalRoot::open(journal).unwrap());
    // Owner deletion recovers both under its own lease, then proceeds.
    let removed = remove_segments(
        journal,
        &[Target {
            day: "20261009".into(),
            stream: "camera".into(),
            dir: "120000_60_suffix".into(),
        }],
        "2026-10-11T00:00:00Z",
        RemovalReason::OwnerSegmentDelete,
        "fixture",
    );
    assert!(removed.targets[0].not_removed.is_empty(), "{removed:?}");
    assert!(!named.exists() && !direct.exists());
    assert!(observe_obligations(&root).unwrap().entries().is_empty());
}

#[test]
fn any_other_stage_without_intent_is_held_untouched() {
    let leaf = super::super::RESTORE_DOWNLOAD_LEAF;
    let readback = crate::archive_approval::READBACK_DOWNLOAD_LEAF;
    for (relative, leaves) in [
        // A shared leaf is also how removal and readback stages look.
        ("chronicle/20261009/.tmp-offload-a", vec!["payload"]),
        ("chronicle/20261009/.tmp-offload-b", vec!["audio.flac"]),
        ("chronicle/20261009/.tmp-offload-c", vec![leaf, "extra"]),
        // The root holds only readback stages, by their own leaf, and offload
        // copies; an older readback stage with the shared leaf, and anything
        // named like a restore or removal, stays held.
        (".tmp-offload-d", vec![leaf]),
        (".tmp-offload-r", vec!["payload"]),
        (".tmp-offload-s", vec![readback, "extra"]),
        (".tmp-offload-u", vec!["audio.flac", "video.webm"]),
        (
            ".tmp-offload-v",
            vec![solstone_core_journal_io::media::REMOVAL_ORIGINAL_LEAF],
        ),
        (
            ".tmp-offload-w",
            vec![solstone_core_journal_io::media::REMOVAL_RECORD_LEAF],
        ),
        ("chronicle/20261009/.tmp-offload-t", vec![readback]),
        // Never inside a segment.
        ("chronicle/20261009/120000_60/.tmp-offload-e", vec![leaf]),
    ] {
        let temp = tempfile::tempdir().unwrap();
        let stage = unrecorded(temp.path(), relative, &leaves);
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
        assert!(
            recover_abandoned_restores(&activity)
                .expect("local recovery")
                .is_empty(),
            "{relative}"
        );
        drop(activity);
        for name in &leaves {
            assert_eq!(fs::read(stage.join(name)).unwrap(), RESTORED, "{relative}");
        }
    }
}

#[test]
fn empty_stage_without_intent_in_a_restore_location_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let day = unrecorded(temp.path(), "chronicle/20261009/.tmp-offload-f", &[]);
    let stream = unrecorded(temp.path(), "chronicle/20261009/camera/.tmp-offload-g", &[]);
    let segment = unrecorded(
        temp.path(),
        "chronicle/20261009/120000_60/.tmp-offload-h",
        &[],
    );
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    assert_eq!(
        recover_abandoned_restores(&activity)
            .expect("local recovery")
            .len(),
        2
    );
    assert!(!day.exists() && !stream.exists());
    // Inside a segment is not where restores stage; it stays held.
    assert!(segment.is_dir());
}

#[test]
fn empty_metadata_stage_after_intent_removal_is_recovered() {
    let temp = tempfile::tempdir().unwrap();
    let empty = unrecorded(temp.path(), "health/offload/.tmp-offload-empty", &[]);
    let retained = unrecorded(
        temp.path(),
        "health/offload/.tmp-offload-retained",
        &[solstone_core_journal_io::media::REMOVAL_ORIGINAL_LEAF],
    );
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    assert_eq!(
        recover_abandoned_restores(&activity)
            .expect("local recovery")
            .len(),
        1
    );
    assert!(!empty.exists());
    assert_eq!(
        fs::read(retained.join(solstone_core_journal_io::media::REMOVAL_ORIGINAL_LEAF)).unwrap(),
        RESTORED
    );
    assert!(
        recover_abandoned_restores(&activity)
            .expect("local recovery")
            .is_empty()
    );
}

#[test]
fn readback_stage_without_intent_at_the_root_is_recovered() {
    let temp = tempfile::tempdir().unwrap();
    let readback = unrecorded(
        temp.path(),
        ".tmp-offload-0123456789abcdef0123456789abcdef",
        &[crate::archive_approval::READBACK_DOWNLOAD_LEAF],
    );
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    let recovered = recover_abandoned_restores(&activity).expect("local recovery");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].outcome, Ok(AbandonedRestoreRelease::Undone));
    assert!(!readback.exists());
    assert!(observe_obligations(&root).unwrap().entries().is_empty());
}

#[test]
fn offload_copy_stage_without_intent_at_the_root_is_recovered() {
    let temp = tempfile::tempdir().unwrap();
    let copy = unrecorded(
        temp.path(),
        ".tmp-offload-0123456789abcdef0123456789abcdef",
        &["audio.flac"],
    );
    let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
    let activity = ActivityLease::try_acquire(Arc::clone(&root)).unwrap();
    let recovered = recover_abandoned_restores(&activity).expect("local recovery");
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].outcome, Ok(AbandonedRestoreRelease::Undone));
    assert!(!copy.exists());
    assert!(observe_obligations(&root).unwrap().entries().is_empty());
}
