// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::process::Command;

use super::*;
use crate::media::{
    Cleanup, Confirmation, MediaDirectory, MediaPrimitive, MediaStage, PreparedMedia,
    PublicationEffect, RemovalEffect, digest_reader, run_with_media_hook,
};

struct Fixture {
    temp: tempfile::TempDir,
    root: Arc<JournalRoot>,
    parent: Arc<MediaDirectory>,
    live: Arc<MediaDirectory>,
}

impl Fixture {
    fn new() -> Self {
        let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        fs::create_dir_all(temp.path().join("parent/segment")).unwrap();
        fs::write(temp.path().join("parent/segment/source.flac"), b"abc").unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let parent = MediaDirectory::open(root.clone(), Path::new("parent")).unwrap();
        let live = parent.open_child(OsStr::new("segment")).unwrap().unwrap();
        Self {
            temp,
            root,
            parent,
            live,
        }
    }

    fn prepared(&self, activity: Arc<ActivityLease>) -> PreparedMedia {
        MediaStage::create(self.parent.clone(), activity)
            .unwrap()
            .create_target()
            .unwrap()
            .copy_from(
                &self.live,
                OsStr::new("source.flac"),
                &digest_reader(&mut &b"abc"[..]).unwrap(),
            )
            .unwrap()
    }

    fn replace_entry(&self) {
        fs::rename(
            self.temp.path().join(ENTRY),
            self.temp.path().join("old.lock"),
        )
        .unwrap();
        fs::write(self.temp.path().join(ENTRY), b"replacement").unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(
                self.temp.path().join(ENTRY),
                fs::Permissions::from_mode(0o600),
            )
            .unwrap();
        }
    }
}

fn busy<T>(result: Result<T, ActivityAdmissionError>) {
    assert!(matches!(result, Err(ActivityAdmissionError::Busy)));
}

#[test]
fn exclusive_roles_are_non_reentrant_and_entry_survives_release() {
    let f = Fixture::new();
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    busy(ActivityLease::try_acquire(f.root.clone()));
    busy(TeardownLease::try_acquire(f.root.clone()));
    let directory = MediaDirectory::open(f.root.clone(), Path::new("")).unwrap();
    let entry = directory
        .entries(4)
        .unwrap()
        .unwrap()
        .into_iter()
        .find(|e| e.name == ENTRY)
        .unwrap();
    drop(activity);
    let teardown = TeardownLease::try_acquire(f.root.clone()).unwrap();
    busy(ActivityLease::try_acquire(f.root.clone()));
    busy(TeardownLease::try_acquire(f.root.clone()));
    teardown.revalidate().unwrap();
    drop(teardown);
    let after = directory
        .entries(4)
        .unwrap()
        .unwrap()
        .into_iter()
        .find(|e| e.name == ENTRY)
        .unwrap();
    assert_eq!(entry.identity, after.identity);
    assert_eq!(fs::read(f.temp.path().join(ENTRY)).unwrap(), b"");
}

#[cfg(target_os = "linux")]
#[test]
fn child_descriptor_keeps_actual_lock_identity_and_refuses_changed_bindings() {
    use std::os::unix::fs::MetadataExt;
    for replace_root in [false, true] {
        let f = Fixture::new();
        let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
        let descriptor = std::fs::File::from(activity.duplicate_descriptor_for_child().unwrap());
        let named = fs::metadata(f.temp.path().join(ENTRY)).unwrap();
        let retained = descriptor.metadata().unwrap();
        assert_eq!((retained.dev(), retained.ino()), (named.dev(), named.ino()));
        busy(ActivityLease::try_acquire(f.root.clone()));
        if replace_root {
            let moved = f.temp.path().with_extension("retained");
            fs::rename(f.temp.path(), &moved).unwrap();
            fs::create_dir(f.temp.path()).unwrap();
            assert!(activity.duplicate_descriptor_for_child().is_err());
            drop(descriptor);
            drop(activity);
            fs::remove_dir_all(moved).unwrap();
        } else {
            f.replace_entry();
            assert!(activity.duplicate_descriptor_for_child().is_err());
            let still_retained = descriptor.metadata().unwrap();
            assert_eq!(
                (still_retained.dev(), still_retained.ino()),
                (named.dev(), named.ino())
            );
        }
    }
}

#[cfg(target_os = "linux")]
#[test]
fn existing_only_admission_retains_guard_after_final_validation_failure_and_never_creates() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    let missing = ActivityLease::try_acquire_existing(f.root.clone())
        .err()
        .unwrap();
    assert!(missing.lease.is_none());
    assert!(!f.temp.path().join(ENTRY).exists());
    drop(ActivityLease::try_acquire(f.root.clone()).unwrap());
    let path = f.temp.path().to_owned();
    let refused = ActivityLease::acquire_existing_with(f.root.clone(), |_| {
        fs::set_permissions(&path, fs::Permissions::from_mode(0o000)).unwrap();
    })
    .err()
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
    assert!(refused.lease.is_some());
    assert!(matches!(
        refused.error,
        ActivityAdmissionError::Unavailable(_)
    ));
    busy(ActivityLease::try_acquire(f.root.clone()));
    let refused = std::thread::spawn(move || refused).join().unwrap();
    busy(ActivityLease::try_acquire(f.root.clone()));
    drop(refused);
    ActivityLease::try_acquire_existing(f.root.clone())
        .ok()
        .unwrap()
        .revalidate()
        .unwrap();
}

#[test]
#[ignore = "invoked only by the owning two-process control"]
fn child_admission_probe() {
    let root_path = std::env::var_os("ACTIVITY_PROBE_ROOT").unwrap();
    let root = Arc::new(JournalRoot::open(Path::new(&root_path)).unwrap());
    let teardown = std::env::var("ACTIVITY_PROBE_ROLE").unwrap() == "teardown";
    let expected_busy = std::env::var("ACTIVITY_PROBE_BUSY").unwrap() == "yes";
    let result = if teardown {
        TeardownLease::try_acquire(root).map(|lease| lease.revalidate().unwrap())
    } else {
        ActivityLease::try_acquire(root).map(|lease| lease.revalidate().unwrap())
    };
    if expected_busy {
        busy(result);
    } else {
        result.unwrap();
    }
    println!("ACTIVITY_PROBE_CONFIRMED");
}

fn child(f: &Fixture, role: &str, expected_busy: bool) {
    let output = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "activity::tests::child_admission_probe",
            "--ignored",
            "--nocapture",
        ])
        .env("ACTIVITY_PROBE_ROOT", f.temp.path())
        .env("ACTIVITY_PROBE_ROLE", role)
        .env(
            "ACTIVITY_PROBE_BUSY",
            if expected_busy { "yes" } else { "no" },
        )
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    assert!(stdout.contains("ACTIVITY_PROBE_CONFIRMED"));
    assert!(stdout.contains("1 passed; 0 failed"), "{stdout}");
}

#[test]
fn two_process_contention_has_successful_instrument_and_release_controls() {
    let f = Fixture::new();
    child(&f, "activity", false);
    child(&f, "teardown", false);
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    child(&f, "activity", true);
    child(&f, "teardown", true);
    drop(activity);
    let teardown = TeardownLease::try_acquire(f.root.clone()).unwrap();
    child(&f, "activity", true);
    child(&f, "teardown", true);
    drop(teardown);
    child(&f, "activity", false);
    child(&f, "teardown", false);
}

#[test]
fn stages_and_failures_retain_original_admission_without_nested_acquire() {
    let f = Fixture::new();
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    let mut first = MediaStage::create(f.parent.clone(), activity.clone()).unwrap();
    assert_eq!(first.cleanup(), Cleanup::Complete);
    let second = MediaStage::create(f.parent.clone(), activity.clone()).unwrap();
    let mut failure = second
        .create_named_target(OsStr::new("../invalid"))
        .err()
        .unwrap();
    drop(first);
    drop(activity);
    busy(TeardownLease::try_acquire(f.root.clone()));
    assert_eq!(failure.owned.cleanup(), Cleanup::Complete);
    busy(TeardownLease::try_acquire(f.root.clone()));
    drop(failure);
    TeardownLease::try_acquire(f.root.clone()).unwrap();
}

#[test]
fn partial_stage_creation_keeps_admission_and_the_actual_reservation() {
    let f = Fixture::new();
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    let (result, fired) = run_with_media_hook(
        MediaPrimitive::StageCreated,
        || {
            Err(std::io::Error::other(
                "injected after actual stage creation",
            ))
        },
        || MediaStage::create(f.parent.clone(), activity.clone()),
    );
    assert!(fired);
    let mut stage = result.err().unwrap().owned.unwrap();
    assert!(stage.diagnostic_path().is_dir());
    drop(activity);
    busy(TeardownLease::try_acquire(f.root.clone()));
    assert_eq!(stage.cleanup(), Cleanup::Complete);
    busy(TeardownLease::try_acquire(f.root.clone()));
    drop(stage);
    TeardownLease::try_acquire(f.root.clone()).unwrap();
}

#[test]
fn separately_admitted_root_and_other_journal_effects_are_refused() {
    let a = Fixture::new();
    let b = Fixture::new();
    let activity = ActivityLease::try_acquire(a.root.clone()).unwrap();
    let reopened = Arc::new(JournalRoot::open(a.temp.path()).unwrap());
    let parent = MediaDirectory::open(reopened, Path::new("parent")).unwrap();
    assert!(
        MediaStage::create(parent, activity.clone())
            .err()
            .unwrap()
            .owned
            .is_none()
    );
    let _teardown = TeardownLease::try_acquire(b.root.clone()).unwrap();
    let mut prepared = a.prepared(activity.clone());
    let receipt = prepared.publish(&b.live, OsStr::new("restored.flac"));
    assert_eq!(receipt.effect, PublicationEffect::NotPublished);
    assert!(!b.live.diagnostic_path().join("restored.flac").exists());
    assert_eq!(
        prepared
            .publish(&a.live, OsStr::new("restored.flac"))
            .effect,
        PublicationEffect::Published
    );
    let expected = b
        .live
        .observe_file(OsStr::new("source.flac"))
        .unwrap()
        .unwrap();
    let stage = MediaStage::create(a.parent.clone(), activity.clone()).unwrap();
    let removal = b
        .live
        .remove_observed(OsStr::new("source.flac"), &expected, stage);
    assert_eq!(removal.receipt().effect, RemovalEffect::NotClaimed);
    assert_eq!(
        fs::read(b.live.diagnostic_path().join("source.flac")).unwrap(),
        b"abc"
    );
    drop(activity);
    drop(prepared);
    busy(TeardownLease::try_acquire(a.root.clone()));
    drop(removal);
    TeardownLease::try_acquire(a.root.clone()).unwrap();
}

#[test]
fn replaced_entry_blocks_pre_effect_boundaries_and_preserves_applied_publication() {
    for after_effect in [false, true] {
        let f = Fixture::new();
        let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
        let mut prepared = f.prepared(activity.clone());
        let entry = f.temp.path().join(ENTRY);
        let old = f.temp.path().join("old.lock");
        let primitive = if after_effect {
            MediaPrimitive::AfterPublish
        } else {
            MediaPrimitive::BeforePublish
        };
        let (receipt, fired) = run_with_media_hook(
            primitive,
            move || {
                fs::rename(&entry, old)?;
                fs::write(&entry, b"replacement")
            },
            || prepared.publish(&f.live, OsStr::new("restored.flac")),
        );
        assert!(fired);
        assert_eq!(
            receipt.effect,
            if after_effect {
                PublicationEffect::Published
            } else {
                PublicationEffect::NotPublished
            }
        );
        assert_eq!(receipt.confirmation, Confirmation::Unverified);
        assert_eq!(prepared.cleanup(), Cleanup::Refused);
        assert!(activity.revalidate().is_err());
        assert_eq!(fs::read(f.temp.path().join(ENTRY)).unwrap(), b"replacement");
        assert_eq!(
            f.live.diagnostic_path().join("restored.flac").exists(),
            after_effect
        );
    }
}

#[test]
fn replaced_entry_blocks_raw_claim_and_cleanup_retains_owned_stage() {
    let f = Fixture::new();
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    let stage = MediaStage::create(f.parent.clone(), activity).unwrap();
    let path = stage.diagnostic_path();
    let expected = f
        .live
        .observe_file(OsStr::new("source.flac"))
        .unwrap()
        .unwrap();
    f.replace_entry();
    let mut removal = f
        .live
        .remove_observed(OsStr::new("source.flac"), &expected, stage);
    assert_eq!(removal.receipt().effect, RemovalEffect::NotClaimed);
    assert_eq!(removal.cleanup(), Cleanup::Refused);
    assert!(path.is_dir());
    assert_eq!(
        fs::read(f.live.diagnostic_path().join("source.flac")).unwrap(),
        b"abc"
    );
}

#[test]
fn entry_loss_after_raw_claim_preserves_effect_and_only_raw_copy() {
    let f = Fixture::new();
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    let stage = MediaStage::create(f.parent.clone(), activity).unwrap();
    let path = stage.diagnostic_path();
    let expected = f
        .live
        .observe_file(OsStr::new("source.flac"))
        .unwrap()
        .unwrap();
    let entry = f.temp.path().join(ENTRY);
    let old = f.temp.path().join("old.lock");
    let (mut removal, fired) = run_with_media_hook(
        MediaPrimitive::AfterRemovalClaim,
        move || {
            fs::rename(&entry, old)?;
            fs::write(entry, b"replacement")
        },
        || {
            f.live
                .remove_observed(OsStr::new("source.flac"), &expected, stage)
        },
    );
    assert!(fired);
    assert_eq!(removal.receipt().effect, RemovalEffect::Claimed);
    assert_eq!(
        removal.receipt().claim_confirmation,
        Confirmation::Unverified
    );
    assert_eq!(removal.cleanup(), Cleanup::Refused);
    assert_eq!(
        fs::read(path.join(crate::media::REMOVAL_ORIGINAL_LEAF)).unwrap(),
        b"abc"
    );
    assert!(!f.live.diagnostic_path().join("source.flac").exists());
}

#[cfg(unix)]
#[test]
fn permission_denial_is_unavailable_and_never_busy() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    fs::set_permissions(f.temp.path(), fs::Permissions::from_mode(0o500)).unwrap();
    let control = fs::write(f.temp.path().join("write-control"), b"control");
    let result = ActivityLease::try_acquire(f.root.clone());
    fs::set_permissions(f.temp.path(), fs::Permissions::from_mode(0o700)).unwrap();
    if control.is_ok() {
        eprintln!("PERMISSION_CONTROL_BYPASSED_BY_HOST; no permission-denial qualification");
        return;
    }
    assert_eq!(
        control.unwrap_err().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert!(matches!(result,
        Err(ActivityAdmissionError::Unavailable(ActivityUnavailable::Entry(ExistingParentLockError::Io { source, .. })))
        if source.kind() == std::io::ErrorKind::PermissionDenied));
    assert!(!f.temp.path().join(ENTRY).exists());
}

#[test]
fn uncooperating_entry_replacement_splits_locks_and_old_validation_refuses() {
    let f = Fixture::new();
    let old = ActivityLease::try_acquire(f.root.clone()).unwrap();
    f.replace_entry();
    let new = TeardownLease::try_acquire(f.root.clone()).unwrap();
    assert!(old.revalidate().is_err());
    new.revalidate().unwrap();
    // This is a documented limit, not a safe teardown certificate.
}

#[test]
fn unsafe_entry_is_unavailable_and_is_never_replaced() {
    let f = Fixture::new();
    fs::create_dir(f.temp.path().join(ENTRY)).unwrap();
    assert!(matches!(
        ActivityLease::try_acquire(f.root.clone()),
        Err(ActivityAdmissionError::Unavailable(_))
    ));
    assert!(f.temp.path().join(ENTRY).is_dir());
}

#[cfg(unix)]
#[test]
fn loose_mode_is_repaired_and_root_rebind_remains_unavailable() {
    use std::os::unix::fs::PermissionsExt;
    let f = Fixture::new();
    fs::write(f.temp.path().join(ENTRY), b"preserve").unwrap();
    fs::set_permissions(f.temp.path().join(ENTRY), fs::Permissions::from_mode(0o644)).unwrap();
    drop(ActivityLease::try_acquire(f.root.clone()).unwrap());
    assert_eq!(
        fs::metadata(f.temp.path().join(ENTRY))
            .unwrap()
            .permissions()
            .mode()
            & 0o7777,
        0o600
    );
    assert_eq!(fs::read(f.temp.path().join(ENTRY)).unwrap(), b"preserve");
    fs::set_permissions(f.temp.path().join(ENTRY), fs::Permissions::from_mode(0o600)).unwrap();
    let activity = ActivityLease::try_acquire(f.root.clone()).unwrap();
    let saved = f.temp.path().with_extension("saved");
    fs::rename(f.temp.path(), &saved).unwrap();
    fs::create_dir(f.temp.path()).unwrap();
    assert!(activity.revalidate().is_err());
    assert!(MediaStage::create(f.parent.clone(), activity).is_err());
    // Restore the fixture namespace only after the refusal; no production cleanup.
    fs::remove_dir(f.temp.path()).unwrap();
    fs::rename(saved, f.temp.path()).unwrap();
}

#[test]
fn actual_admissions_fit_send_sync_resource_ownership() {
    fn require<T: Send + Sync>() {}
    require::<ActivityLease>();
    require::<TeardownLease>();
}
