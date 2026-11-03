// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;
use crate::atomic::{
    BoundPublicationPrimitive, run_with_bound_publication_barrier, run_with_bound_publication_fault,
};
use crate::{BoundAtomicOutcome, FlatDirectory, JournalRoot};
use std::time::Duration;

fn parent(root: &tempfile::TempDir) -> FlatDirectory {
    fs::create_dir(root.path().join("metadata")).unwrap();
    let root = JournalRoot::open(root.path()).unwrap();
    FlatDirectory::open(&root, Path::new("metadata")).unwrap()
}
fn immediate() -> LockOptions {
    LockOptions {
        timeout: Duration::ZERO,
        poll_interval: Duration::from_millis(1),
        ..LockOptions::default()
    }
}
fn confirmed(publication: &BoundStreamMarkerPublication, generation: u64) {
    assert_eq!(publication.generation(), generation);
    assert!(matches!(
        publication.publication(),
        BoundAtomicOutcome::Published { .. }
    ));
    assert!(publication.binding_error().is_none());
    assert!(publication.lock_error().is_none());
}

#[test]
fn bound_marker_absent_empty_versioned_and_fingerprint() {
    use std::os::unix::fs::PermissionsExt;
    for initial in [
        None,
        Some(Vec::new()),
        Some(br#"{"version":1,"generation":7,"fingerprint":"exact","future":true}"#.to_vec()),
    ] {
        let root = tempfile::tempdir().unwrap();
        let parent = parent(&root);
        let path = root.path().join("metadata/stream.updated");
        if let Some(bytes) = &initial {
            fs::write(&path, bytes).unwrap();
        }
        let expected = if initial.as_ref().is_some_and(|b| !b.is_empty()) {
            8
        } else {
            1
        };
        confirmed(
            &bump_stream_marker_bound(&parent, immediate()).unwrap(),
            expected,
        );
        assert_eq!(
            fs::metadata(root.path().join("metadata/stream.updated.lock"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let marker: HealthMarker = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
        assert_eq!(marker.generation, expected);
        assert_eq!(
            marker.fingerprint,
            if expected == 8 {
                Some("exact".to_owned())
            } else {
                None
            }
        );
        confirmed(
            &bump_stream_marker_bound(&parent, immediate()).unwrap(),
            expected + 1,
        );
    }
}

#[test]
fn bound_marker_malformed_version_overflow_and_symlink_preserve_input() {
    for (bytes, overflow) in [
        (b"{".as_slice(), false),
        (br#"{"version":2,"generation":0,"fingerprint":null}"#, false),
        (
            br#"{"version":1,"generation":18446744073709551615,"fingerprint":null}"#,
            true,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let parent = parent(&root);
        let path = root.path().join("metadata/stream.updated");
        fs::write(&path, bytes).unwrap();
        let error = bump_stream_marker_bound(&parent, immediate()).unwrap_err();
        if overflow {
            assert!(matches!(error, BoundStreamMarkerError::Overflow));
        } else {
            assert!(matches!(error, BoundStreamMarkerError::Malformed));
        }
        assert_eq!(fs::read(&path).unwrap(), bytes);
        assert_eq!(fs::read_dir(path.parent().unwrap()).unwrap().count(), 2);
    }
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    fs::write(root.path().join("outside"), b"outside").unwrap();
    std::os::unix::fs::symlink(
        root.path().join("outside"),
        root.path().join("metadata/stream.updated"),
    )
    .unwrap();
    assert!(matches!(
        bump_stream_marker_bound(&parent, immediate()),
        Err(BoundStreamMarkerError::Read(_))
    ));
    assert_eq!(fs::read(root.path().join("outside")).unwrap(), b"outside");
}

#[test]
fn bound_marker_shares_existing_path_lock_in_both_directions() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    let path = root.path().join("metadata/stream.updated");
    fs::write(&path, b"").unwrap();
    {
        let _held = hold_lock(&path, immediate()).unwrap();
        fs::set_permissions(
            path.with_file_name("stream.updated.lock"),
            fs::Permissions::from_mode(0o644),
        )
        .unwrap();
        let result = bump_stream_marker_bound(&parent, immediate());
        assert!(
            matches!(
                &result,
                Err(BoundStreamMarkerError::Lock(
                    crate::ExistingParentLockError::Timeout { .. }
                ))
            ),
            "{result:?}"
        );
        assert_eq!(fs::read(&path).unwrap(), b"");
    }
    let lock_path = path.with_file_name("stream.updated.lock");
    let before = fs::metadata(&lock_path).unwrap();
    let mut checked = false;
    let result = bump_bound_with_checkpoint(&parent, immediate(), |point| {
        if point == MarkerPoint::BeforePublication {
            checked = true;
            assert!(matches!(
                hold_lock(&path, immediate()),
                Err(crate::LockError::Timeout { .. })
            ));
            assert_eq!(fs::read(&path).unwrap(), b"");
        }
    })
    .unwrap();
    assert!(checked);
    confirmed(&result, 1);
    let after = fs::metadata(&lock_path).unwrap();
    assert_eq!(
        (before.dev(), before.ino(), before.mode()),
        (after.dev(), after.ino(), after.mode())
    );
}

#[test]
fn bound_lock_existing_mode_revalidates_and_replacement_refuses() {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    let path = root.path().join("metadata/stream.updated.lock");
    fs::write(&path, b"legacy").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let name = OsStr::new("stream.updated.lock");
    let held = crate::open_existing_parent_lock_bound(
        &parent,
        name,
        Duration::ZERO,
        Duration::from_millis(1),
    )
    .unwrap()
    .unwrap();
    held.revalidate_bound(&parent, name).unwrap();
    fs::rename(&path, root.path().join("original.lock")).unwrap();
    fs::write(&path, b"replacement").unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        held.revalidate_bound(&parent, name),
        Err(crate::ExistingParentLockError::NamespaceChanged { .. })
    ));
    assert_eq!(fs::read(&path).unwrap(), b"replacement");
    assert_eq!(
        fs::read(root.path().join("original.lock")).unwrap(),
        b"legacy"
    );
}

#[test]
fn bound_lock_revalidation_repairs_loose_modes() {
    use std::ffi::OsStr;
    use std::os::unix::fs::PermissionsExt;
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    let name = OsStr::new("stream.updated.lock");
    let held = crate::acquire_existing_parent_lock_bound(
        &parent,
        name,
        Duration::ZERO,
        Duration::from_millis(1),
    )
    .unwrap();
    held.revalidate_bound(&parent, name).unwrap();
    let path = root.path().join("metadata/stream.updated.lock");
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    held.revalidate_bound(&parent, name).unwrap();
    assert_eq!(
        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
        0o600
    );
    assert!(fs::read(&path).unwrap().is_empty());
}

#[test]
fn bound_marker_lock_replacement_preserves_pre_and_post_effects() {
    for point in [
        MarkerPoint::BeforePublication,
        MarkerPoint::AfterPublication,
    ] {
        let root = tempfile::tempdir().unwrap();
        let parent = parent(&root);
        let path = root.path().join("metadata/stream.updated");
        fs::write(&path, b"").unwrap();
        let mut fired = false;
        let result = bump_bound_with_checkpoint(&parent, immediate(), |current| {
            if point == current {
                fired = true;
                fs::rename(
                    root.path().join("metadata/stream.updated.lock"),
                    root.path().join("old-lock"),
                )
                .unwrap();
                fs::write(root.path().join("metadata/stream.updated.lock"), b"").unwrap();
            }
        });
        assert!(fired);
        match point {
            MarkerPoint::BeforePublication => {
                assert!(matches!(result, Err(BoundStreamMarkerError::Lock(_))));
                assert_eq!(fs::read(&path).unwrap(), b"");
            }
            MarkerPoint::AfterPublication => {
                let result = result.unwrap();
                assert_eq!(result.generation(), 1);
                assert!(matches!(
                    result.publication(),
                    BoundAtomicOutcome::Published { .. }
                ));
                assert!(result.lock_error().is_some());
            }
        }
    }
}

#[test]
fn bound_marker_actual_atomic_uncertainty_survives_lock_error() {
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    let (result, consumed) = run_with_bound_publication_fault(
        BoundPublicationPrimitive::ParentSync,
        1,
        nix::libc::EIO,
        || {
            bump_bound_with_checkpoint(&parent, immediate(), |point| {
                if point == MarkerPoint::AfterPublication {
                    fs::rename(
                        root.path().join("metadata/stream.updated.lock"),
                        root.path().join("old-lock"),
                    )
                    .unwrap();
                    fs::write(root.path().join("metadata/stream.updated.lock"), b"").unwrap();
                }
            })
        },
    );
    assert!(consumed);
    let result = result.unwrap();
    assert_eq!(result.generation(), 1);
    assert!(matches!(
        result.publication(),
        BoundAtomicOutcome::PublishedDurabilityUncertain { .. }
    ));
    assert!(result.lock_error().is_some());
}

#[test]
fn bound_marker_atomic_failure_keeps_previous_marker() {
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    let path = root.path().join("metadata/stream.updated");
    let before = br#"{"version":1,"generation":7,"fingerprint":null}"#;
    fs::write(&path, before).unwrap();
    let (result, consumed) = run_with_bound_publication_fault(
        BoundPublicationPrimitive::Write,
        1,
        nix::libc::EIO,
        || bump_stream_marker_bound(&parent, immediate()),
    );
    assert!(consumed);
    assert!(matches!(result, Err(BoundStreamMarkerError::Publish(_))));
    assert_eq!(fs::read(path).unwrap(), before);
}

#[test]
fn bound_marker_ancestor_replacement_cannot_redirect_publication() {
    let root = tempfile::tempdir().unwrap();
    let parent = parent(&root);
    let original = root.path().join("original");
    let path = root.path().join("metadata");
    let barrier_path = path.clone();
    let barrier_original = original.clone();
    let (result, fired) = run_with_bound_publication_barrier(
        BoundPublicationPrimitive::Rename,
        1,
        move || {
            fs::rename(&barrier_path, &barrier_original).unwrap();
            fs::create_dir(&barrier_path).unwrap();
            fs::write(barrier_path.join("stream.updated"), b"competitor").unwrap();
        },
        || bump_stream_marker_bound(&parent, immediate()),
    );
    assert!(fired);
    confirmed(&result.unwrap(), 1);
    assert_eq!(
        fs::read(path.join("stream.updated")).unwrap(),
        b"competitor"
    );
    let marker: HealthMarker =
        serde_json::from_slice(&fs::read(original.join("stream.updated")).unwrap()).unwrap();
    assert_eq!(marker.generation, 1);
}
