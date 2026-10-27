// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::cli_restore::{CliRestoreSettlement, run_cli_restore};
use solstone_core_backup::archive_admission::Clock;
use solstone_core_journal_io::activity_obligations::observe_obligations;
use std::path::Path;

struct FixedClock;
impl Clock for FixedClock {
    fn now_unix(&self) -> i64 {
        3
    }
    fn iso_week(&self) -> u8 {
        41
    }
}

fn completed_fixture() -> (Fixture, PathBuf) {
    let f = Fixture::new(false);
    let allowed = Cell::new(true);
    let outcome = f.staged(&allowed).commit(2, options());
    assert!(outcome.hold.is_none(), "{:?}", outcome.hold);
    let intent = &outcome.record.as_ref().unwrap().intent;
    assert_eq!(
        intent.retirement,
        solstone_core_retention::restore_intent::MarkRetirement::NoMark
    );
    assert_eq!(intent.register, Obligation::NotRequired);
    assert_eq!(intent.event_recorded, Obligation::Complete);
    assert_eq!(intent.marker, Obligation::Complete);
    assert_eq!(intent.cleanup, Obligation::Complete);
    assert!(intent.recovery_binding.is_none());
    let operation_id = intent.event.operation_id.clone();
    let name =
        solstone_core_journal_io::activity_obligations::intent_filename(&operation_id).unwrap();
    let path = f
        .temp
        .path()
        .join("health/offload/restore-intents")
        .join(name);
    assert!(path.is_file());
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    drop(outcome);
    (f, path)
}

#[test]
fn ordinary_cli_retires_completed_no_mark_before_empty_selection() {
    let (f, path) = completed_fixture();
    let ledger = fs::read(f.ledger()).unwrap();
    let catalog = read_complete_catalog(&f.root).unwrap();
    assert!(
        catalog
            .days
            .iter()
            .flat_map(|day| &day.segments)
            .all(|segment| segment.active.is_empty())
    );
    let run = run_cli_restore(f.temp.path(), Arc::new(FixedClock), None);
    assert_eq!(run.result().status, "no_op", "{:?}", run.result());
    assert_eq!(run.result().reason.as_deref(), Some("nothing_to_restore"));
    assert_eq!(run.settlement(), CliRestoreSettlement::NotStarted);
    assert_eq!(run.result().files_restored, 0);
    assert_eq!(run.result().bytes_restored, 0);
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
    assert_eq!(read_complete_catalog(&f.root).unwrap(), catalog);
    assert!(
        !path.exists(),
        "ordinary CLI left the completed NoMark intent"
    );
    assert!(observe_obligations(&f.root).unwrap().entries().is_empty());
    assert!(!f.temp.path().join("health/retention-marks.json").exists());
}

#[test]
fn ordinary_cli_refuses_mixed_obligations_before_recovery_effects() {
    use solstone_core_journal_io::media::{MediaStage, REMOVAL_ORIGINAL_LEAF, RemovalEffect};
    for interrupted in [false, true] {
        let (f, path) = completed_fixture();
        let original = fs::read(&path).unwrap();
        let ledger = fs::read(f.ledger()).unwrap();
        let mut staged_original = None;
        if interrupted {
            fs::write(f.temp.path().join("source/foreign.bin"), b"foreign").unwrap();
            let activity = ActivityLease::try_acquire(Arc::clone(&f.root)).unwrap();
            let source = MediaDirectory::open(Arc::clone(&f.root), Path::new("source")).unwrap();
            let parent = MediaDirectory::open(Arc::clone(&f.root), Path::new("")).unwrap();
            let stage = MediaStage::create(parent, activity).unwrap();
            let actual_path = stage.diagnostic_path().join(REMOVAL_ORIGINAL_LEAF);
            let expected = source
                .observe_file(OsStr::new("foreign.bin"))
                .unwrap()
                .unwrap();
            let (removal, consumed) = run_with_media_hook(
                MediaPrimitive::AfterRemovalClaim,
                || Err(std::io::Error::other("interrupted original removal")),
                || source.remove_observed(OsStr::new("foreign.bin"), &expected, stage),
            );
            assert!(consumed);
            assert_eq!(removal.receipt().effect, RemovalEffect::Claimed);
            assert_eq!(fs::read(&actual_path).unwrap(), b"foreign");
            drop(removal);
            staged_original = Some(actual_path);
        } else {
            fs::write(
                path.parent().unwrap().join("unknown.json"),
                b"not an intent",
            )
            .unwrap();
        }
        let census = observe_obligations(&f.root).unwrap().into_entries();
        let run = run_cli_restore(f.temp.path(), Arc::new(FixedClock), None);
        assert_eq!(run.result().status, "refused");
        assert_eq!(run.result().reason.as_deref(), Some("restore_held"));
        assert_eq!(run.settlement(), CliRestoreSettlement::Held);
        assert_eq!(fs::read(&path).unwrap(), original);
        assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
        assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
        assert_eq!(observe_obligations(&f.root).unwrap().into_entries(), census);
        assert!(run.local_native_removals().is_empty());
        assert!(run.local_native_creations().is_empty());
        if let Some(path) = staged_original {
            assert_eq!(fs::read(path).unwrap(), b"foreign");
        }
    }
}

#[test]
fn ordinary_cli_retains_actual_creation_failure_before_intent_removal() {
    let (f, path) = completed_fixture();
    let original = fs::read(&path).unwrap();
    let ledger = fs::read(f.ledger()).unwrap();
    let (run, consumed) = run_with_media_hook(
        MediaPrimitive::StageCreated,
        || Err(std::io::ErrorKind::PermissionDenied.into()),
        || run_cli_restore(f.temp.path(), Arc::new(FixedClock), None),
    );
    assert!(consumed);
    assert_eq!(run.settlement(), CliRestoreSettlement::Held);
    assert!(run.local_native_removals().is_empty());
    let creations = run.local_native_creations();
    assert_eq!(creations.len(), 1);
    assert_eq!(
        creations[0].error().unwrap().kind(),
        std::io::ErrorKind::PermissionDenied
    );
    assert_eq!(fs::read(&path).unwrap(), original);
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
    drop(run);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_ok());
}

#[test]
fn ordinary_cli_retains_native_metadata_effect_after_cleanup_failure_or_unwind() {
    use solstone_core_journal_io::media::RemovalEffect;
    for unwinding in [false, true] {
        let (f, path) = completed_fixture();
        let ledger = fs::read(f.ledger()).unwrap();
        let (run, consumed) = run_with_media_hook(
            if unwinding {
                MediaPrimitive::AfterClaimDelete
            } else {
                MediaPrimitive::StageRemove
            },
            move || {
                assert!(!unwinding, "after actual intent deletion");
                Err(std::io::ErrorKind::PermissionDenied.into())
            },
            || run_cli_restore(f.temp.path(), Arc::new(FixedClock), None),
        );
        // An unwind clears the hook guard; it is still consumed before the
        // retained CLI catches the actor, so the returned owner remains inspectable.
        assert!(consumed);
        assert_eq!(
            run.settlement(),
            if unwinding {
                CliRestoreSettlement::Unknown
            } else {
                CliRestoreSettlement::Held
            }
        );
        assert!(!path.exists());
        let removals = run.local_native_removals();
        assert_eq!(removals.len(), 1);
        let receipt = removals[0].receipt();
        assert_eq!(
            receipt.effect,
            if unwinding {
                RemovalEffect::DeletePending
            } else {
                RemovalEffect::Deleted
            }
        );
        assert_ne!(receipt.cleanup, Cleanup::Complete);
        let stage_name = removals[0]
            .stage_observation()
            .unwrap()
            .directory_name
            .clone();
        assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
        assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
        assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
        // Windows retains exclusive native handles until this owner closes.
        // Inspect the persisted stage through a fresh reader only afterward.
        drop(run);
        let health =
            MediaDirectory::open(Arc::clone(&f.root), Path::new("health/offload")).unwrap();
        let stage = health.open_child(OsStr::new(&stage_name)).unwrap().unwrap();
        let contents = stage.entries(1).unwrap().unwrap();
        if unwinding {
            assert_eq!(contents.len(), 1);
            assert_eq!(
                contents[0].name,
                OsStr::new(solstone_core_journal_io::media::REMOVAL_RECORD_LEAF)
            );
        } else {
            assert!(
                contents.is_empty(),
                "only the empty directory retirement failed"
            );
        }
        drop(stage);
        drop(health);
        assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
        assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
        // A cold retry must hold an unqualified nonempty claim. A freshly
        // observed empty orphan needs no reconstructed claim authority.
        let retry = run_cli_restore(f.temp.path(), Arc::new(FixedClock), None);
        if unwinding {
            assert_eq!(retry.result().reason.as_deref(), Some("restore_held"));
            assert_ne!(retry.result().status, "no_op");
        } else {
            assert_eq!(retry.result().reason.as_deref(), Some("nothing_to_restore"));
            assert_eq!(retry.result().status, "no_op");
        }
        assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    }
}

#[test]
fn ordinary_cli_cancellation_after_metadata_effect_retains_the_native_owner() {
    use solstone_core_backup::archive_admission::ArchiveCancellation;
    use solstone_core_journal_io::media::RemovalEffect;
    let (f, path) = completed_fixture();
    let ledger = fs::read(f.ledger()).unwrap();
    let cancellation = ArchiveCancellation::new();
    let signal = cancellation.clone();
    let (run, consumed) = run_with_media_hook(
        MediaPrimitive::AfterClaimDelete,
        move || {
            signal.cancel();
            Ok(())
        },
        || {
            crate::cli_restore::run_cli_restore_with_cancellation(
                f.temp.path(),
                Arc::new(FixedClock),
                None,
                cancellation,
            )
        },
    );
    assert!(consumed);
    assert_eq!(run.settlement(), CliRestoreSettlement::Held);
    assert_eq!(run.result().reason_detail.as_deref(), Some("cancelled"));
    let removals = run.local_native_removals();
    assert_eq!(removals.len(), 1);
    assert_eq!(removals[0].receipt().effect, RemovalEffect::Deleted);
    assert_ne!(removals[0].receipt().cleanup, Cleanup::Complete);
    assert!(!path.exists());
    assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
    assert_eq!(fs::read(f.ledger()).unwrap(), ledger);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_err());
    drop(run);
    assert!(ActivityLease::try_acquire(Arc::clone(&f.root)).is_ok());
}

#[test]
fn ordinary_cli_does_not_adopt_an_owed_mark() {
    let f = Fixture::new(true);
    let allowed = Cell::new(true);
    let outcome = f.staged(&allowed).commit(2, options());
    let record = outcome.record.as_ref().unwrap();
    assert!(matches!(
        record.intent.retirement,
        solstone_core_retention::restore_intent::MarkRetirement::Exact { .. }
    ));
    assert_eq!(record.intent.register, Obligation::Pending);
    let path = f.temp.path().join("health/offload/restore-intents").join(
        solstone_core_journal_io::activity_obligations::intent_filename(
            &record.intent.event.operation_id,
        )
        .unwrap(),
    );
    let original = fs::read(&path).unwrap();
    let register = fs::read(f.temp.path().join("health/retention-marks.json")).unwrap();
    drop(outcome);
    let census = observe_obligations(&f.root).unwrap().into_entries();
    let run = run_cli_restore(f.temp.path(), Arc::new(FixedClock), None);
    assert_eq!(run.result().reason.as_deref(), Some("restore_held"));
    assert_eq!(run.settlement(), CliRestoreSettlement::Held);
    assert_eq!(fs::read(path).unwrap(), original);
    assert_eq!(
        fs::read(f.temp.path().join("health/retention-marks.json")).unwrap(),
        register
    );
    assert_eq!(observe_obligations(&f.root).unwrap().into_entries(), census);
    assert!(run.local_native_removals().is_empty());
}

#[test]
fn ordinary_cli_refuses_busy_activity_and_pending_whole_restore() {
    use solstone_core_journal_io::whole_journal_restore::{
        hold_target_for_restore, release_target_for_restore,
    };
    for pending in [false, true] {
        let (f, path) = completed_fixture();
        let original = fs::read(&path).unwrap();
        let activity = if pending {
            hold_target_for_restore(f.temp.path());
            None
        } else {
            Some(ActivityLease::try_acquire(Arc::clone(&f.root)).unwrap())
        };
        struct Release(PathBuf, bool);
        impl Drop for Release {
            fn drop(&mut self) {
                if self.1 {
                    release_target_for_restore(&self.0);
                }
            }
        }
        let _release = Release(f.temp.path().to_owned(), pending);
        let run = run_cli_restore(f.temp.path(), Arc::new(FixedClock), None);
        assert_eq!(run.result().reason.as_deref(), Some("restore_held"));
        assert_eq!(run.settlement(), CliRestoreSettlement::NotStarted);
        assert_eq!(fs::read(path).unwrap(), original);
        assert_eq!(fs::read(f.raw()).unwrap(), b"abc");
        assert!(run.local_native_removals().is_empty());
        drop(activity);
    }
}

#[cfg(unix)]
#[test]
fn ordinary_cli_never_reopens_a_replaced_root_before_activity_admission() {
    let (f, path) = completed_fixture();
    let relative_intent = path.strip_prefix(f.temp.path()).unwrap().to_owned();
    let original_intent = fs::read(path).unwrap();
    let parking = tempfile::tempdir().unwrap();
    let parked = parking.path().join("original");
    let root_path = f.temp.path().to_owned();
    let old_root = root_path.clone();
    let moved = parked.clone();
    let (run, consumed) = run_with_media_hook(
        MediaPrimitive::AfterFirstEnumeration,
        move || {
            fs::rename(&old_root, &moved)?;
            fs::create_dir(&old_root)?;
            fs::write(old_root.join("sentinel"), b"successor")
        },
        || run_cli_restore(&root_path, Arc::new(FixedClock), None),
    );
    assert!(consumed);
    assert_eq!(run.result().reason.as_deref(), Some("restore_held"));
    assert_ne!(run.result().status, "no_op");
    assert_eq!(fs::read(root_path.join("sentinel")).unwrap(), b"successor");
    let entries: Vec<_> = fs::read_dir(&root_path)
        .unwrap()
        .map(|entry| entry.unwrap().file_name())
        .collect();
    assert_eq!(entries, vec![std::ffi::OsString::from("sentinel")]);
    assert_eq!(
        fs::read(parked.join(relative_intent)).unwrap(),
        original_intent
    );
}
