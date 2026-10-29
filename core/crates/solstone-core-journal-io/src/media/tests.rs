// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use super::*;

#[test]
fn streaming_retries_interrupted_and_matches_million_a_vector() {
    struct Generated {
        remaining: usize,
        interrupt: bool,
    }
    impl Read for Generated {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            assert!(buffer.len() <= BUFFER_BYTES);
            if self.interrupt {
                self.interrupt = false;
                return Err(io::ErrorKind::Interrupted.into());
            }
            let count = self.remaining.min(buffer.len());
            buffer[..count].fill(b'a');
            self.remaining -= count;
            Ok(count)
        }
    }
    let digest = digest_reader(&mut Generated {
        remaining: 1_000_000,
        interrupt: true,
    })
    .unwrap();
    assert_eq!(digest.size, 1_000_000);
    assert_eq!(
        digest.sha256,
        "cdc76e5c9914fb9281a1c7e284d73e67f1809a48a497200e046d39ccc7112cd0"
    );
}

#[test]
fn full_identity_is_lossless_and_strict() {
    let a = MediaIdentity::Windows {
        volume: 42,
        file_id: [0; 16],
    };
    let mut bits = [0; 16];
    bits[0] = 1;
    bits[8] = 1;
    let b = MediaIdentity::Windows {
        volume: 42,
        file_id: bits,
    };
    assert_ne!(a, b); // These IDs collide under xor-of-halves folding.
    let encoded = serde_json::to_string(&b).unwrap();
    assert_eq!(serde_json::from_str::<MediaIdentity>(&encoded).unwrap(), b);
    assert!(
        serde_json::from_str::<MediaIdentity>(
            r#"{"platform":"unix","device":1,"inode":2,"extra":true}"#
        )
        .is_err()
    );
    assert_eq!(checked_length(u64::MAX - 1, 1).unwrap(), u64::MAX);
    assert!(checked_length(u64::MAX, 1).is_err());
    let original = MediaEntry {
        name: "record".into(),
        kind: crate::journal_root::JournalEntryKind::RegularFile,
        identity: a,
        size: 0,
        stamp: Default::default(),
    };
    let mut replaced = original.clone();
    replaced.identity = b;
    assert_ne!(original, replaced);
}

#[cfg(unix)]
#[test]
fn portable_name_validation_refuses_non_utf8() {
    use std::os::unix::ffi::OsStringExt;
    let name = OsString::from_vec(vec![b'b', 0xff]);
    assert_eq!(
        plain_name(&name).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[cfg(unix)]
#[test]
fn publish_directory_noreplace_leaves_an_empty_directory() {
    let root = tempfile::tempdir().unwrap();
    let parent = root.path();
    std::fs::create_dir(parent.join("source")).unwrap();
    std::fs::write(parent.join("source/keep.txt"), b"source").unwrap();
    std::fs::create_dir(parent.join("taken")).unwrap();
    let error =
        publish_directory_noreplace(parent, OsStr::new("source"), OsStr::new("taken")).unwrap_err();
    assert_eq!(error.kind(), io::ErrorKind::AlreadyExists);
    assert!(parent.join("taken").is_dir());
    assert_eq!(std::fs::read_dir(parent.join("taken")).unwrap().count(), 0);
    assert_eq!(
        std::fs::read(parent.join("source/keep.txt")).unwrap(),
        b"source"
    );
}

#[cfg(all(test, feature = "full-tests"))]
mod observations {
    use super::*;
    use crate::journal_root::JournalEntryKind;
    use std::fs;

    fn fixture() -> (tempfile::TempDir, Arc<MediaDirectory>) {
        let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        };
        fs::create_dir(temp.path().join("parent")).unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let parent = MediaDirectory::open(root, Path::new("parent")).unwrap();
        (temp, parent)
    }

    #[test]
    fn complete_listing_and_full_read_are_lossless_prefix_allows_long_files() {
        let (temp, parent) = fixture();
        let dir = temp.path().join("parent");
        assert_eq!(parent.entries(0).unwrap(), Some(vec![]));
        assert!(parent.open_child(OsStr::new("missing")).unwrap().is_none());
        assert!(!dir.join("missing").exists());
        fs::write(dir.join("record.jsonl"), b"abcdefghijk").unwrap();
        fs::write(dir.join(".hidden.flac"), b"hidden").unwrap();
        fs::create_dir(dir.join("child")).unwrap();
        fs::create_dir(dir.join(".tmp-offload-unknown")).unwrap();
        let entries = parent.entries(4).unwrap().unwrap();
        assert_eq!(
            entries
                .iter()
                .map(|entry| entry.name.to_str().unwrap())
                .collect::<Vec<_>>(),
            [
                ".hidden.flac",
                ".tmp-offload-unknown",
                "child",
                "record.jsonl"
            ]
        );
        assert_eq!(entries[1].kind, JournalEntryKind::Directory);
        assert!(parent.entries(3).unwrap().is_none());
        assert!(parent.entries(0).unwrap().is_none());
        let child = parent.open_child(OsStr::new("child")).unwrap().unwrap();
        assert!(child.entries(0).unwrap().unwrap().is_empty());
        assert!(parent.open_child(OsStr::new("record.jsonl")).is_err());
        assert!(
            parent
                .read_file_bounded(OsStr::new("record.jsonl"), 10)
                .is_err()
        );
        let complete = parent
            .read_file_bounded(OsStr::new("record.jsonl"), 11)
            .unwrap()
            .unwrap();
        assert_eq!(complete.entry, entries[3]);
        assert!(complete.complete);
        assert_eq!(complete.bytes, b"abcdefghijk");
        let prefix = parent
            .read_prefix(OsStr::new("record.jsonl"), 3)
            .unwrap()
            .unwrap();
        assert_eq!(prefix.entry, entries[3]);
        assert_eq!(prefix.bytes, b"abc");
        assert!(!prefix.complete);
        let zero = parent
            .read_prefix(OsStr::new("record.jsonl"), 0)
            .unwrap()
            .unwrap();
        assert!(zero.bytes.is_empty() && !zero.complete);
        assert!(
            parent
                .read_file_bounded(OsStr::new("absent"), 10)
                .unwrap()
                .is_none()
        );
        assert!(parent.read_prefix(OsStr::new("child"), 10).is_err());
        #[cfg(windows)]
        if std::env::var("SOLSTONE_MEDIA_TEST_ROOT")
            .unwrap_or_default()
            .starts_with("R:")
        {
            let MediaIdentity::Windows { file_id, .. } = complete.entry.identity else {
                panic!("native Windows identity")
            };
            assert!(file_id[8..].iter().any(|byte| *byte != 0));
        }
    }

    #[test]
    fn membership_changes_between_walks_refuse_without_parent_replacement() {
        for mutation in ["add", "remove", "replace"] {
            let (temp, parent) = fixture();
            let dir = temp.path().join("parent");
            fs::write(dir.join("record"), b"old").unwrap();
            let identity = parent.identity();
            let (result, fired) = run_with_media_hook(
                MediaPrimitive::AfterFirstEnumeration,
                move || {
                    match mutation {
                        "add" => fs::write(dir.join("before-record"), b"new")?,
                        "remove" => fs::remove_file(dir.join("record"))?,
                        _ => {
                            fs::rename(dir.join("record"), dir.join("moved"))?;
                            fs::write(dir.join("record"), b"old")?;
                        }
                    }
                    Ok(())
                },
                || parent.entries(10),
            );
            assert!(fired && result.is_err(), "{mutation}");
            assert_eq!(parent.identity(), identity);
        }
    }

    #[test]
    fn census_listing_lets_ordinary_entries_move_but_not_evidence_or_membership() {
        // Every mutation changes a size, an identity or the membership, never
        // only a time, so a coarse filesystem clock cannot hide it.
        let listed = |mutation: &'static str, census: bool| {
            let (temp, parent) = fixture();
            let dir = temp.path().join("parent");
            fs::write(dir.join("log"), b"old").unwrap();
            fs::write(dir.join("stage"), b"old").unwrap();
            let (result, fired) = run_with_media_hook(
                MediaPrimitive::AfterFirstEnumeration,
                move || {
                    match mutation {
                        "ordinary" => fs::write(dir.join("log"), b"older")?,
                        "evidence" => fs::write(dir.join("stage"), b"older")?,
                        "replace" => {
                            fs::write(dir.join("other"), b"new")?;
                            fs::rename(dir.join("other"), dir.join("log"))?
                        }
                        "remove" => fs::remove_file(dir.join("log"))?,
                        _ => fs::write(dir.join("added"), b"new")?,
                    }
                    Ok(())
                },
                || {
                    if census {
                        parent.census_entries(10, |entry| entry.name == "stage")
                    } else {
                        parent.entries(10)
                    }
                },
            );
            assert!(fired, "{mutation}");
            result.is_ok()
        };
        for (mutation, census_accepts) in [
            ("ordinary", true),
            ("evidence", false),
            ("replace", false),
            ("remove", false),
            ("add", false),
        ] {
            assert_eq!(listed(mutation, true), census_accepts, "census {mutation}");
            assert!(!listed(mutation, false), "strict {mutation}");
        }
    }

    #[test]
    fn late_child_observation_errors_never_become_known_absence() {
        for barrier in [
            MediaPrimitive::AfterChildOpen,
            MediaPrimitive::AfterChildObservation,
        ] {
            let (temp, parent) = fixture();
            fs::create_dir(temp.path().join("parent/child")).unwrap();
            let (result, fired) = run_with_media_hook(
                barrier,
                || Err(io::ErrorKind::NotFound.into()),
                || parent.open_child(OsStr::new("child")),
            );
            assert!(fired);
            assert_eq!(result.err().unwrap().kind(), io::ErrorKind::NotFound);
            parent.revalidate().unwrap();
            assert!(parent.open_child(OsStr::new("child")).unwrap().is_some());
        }
    }

    #[cfg(unix)]
    #[test]
    fn late_child_disappearance_is_uncertain_with_unchanged_parent() {
        for barrier in [
            MediaPrimitive::AfterChildOpen,
            MediaPrimitive::AfterChildObservation,
        ] {
            let (temp, parent) = fixture();
            let child = temp.path().join("parent/child");
            fs::create_dir(&child).unwrap();
            let identity = parent.identity();
            let (result, fired) = run_with_media_hook(
                barrier,
                move || fs::remove_dir(child),
                || parent.open_child(OsStr::new("child")),
            );
            assert!(fired && result.is_err(), "{barrier:?}");
            parent.revalidate().unwrap();
            assert_eq!(parent.identity(), identity);
            assert!(parent.open_child(OsStr::new("child")).unwrap().is_none());
        }
    }

    #[cfg(unix)]
    #[test]
    fn malformed_enumerated_names_refuse_complete_coverage() {
        for name in ["bad:name", "bad\\name"] {
            let (temp, parent) = fixture();
            fs::write(temp.path().join("parent").join(name), b"untouched").unwrap();
            assert!(parent.entries(10).is_err());
            parent.revalidate().unwrap();
        }
    }

    #[test]
    fn replacement_between_directory_record_and_leaf_observation_refuses() {
        let (temp, parent) = fixture();
        let dir = temp.path().join("parent");
        fs::write(dir.join("record"), b"abc").unwrap();
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::BeforeEntryObservation,
            move || {
                fs::rename(dir.join("record"), dir.join("old"))?;
                fs::write(dir.join("record"), b"abc")
            },
            || parent.entries(10),
        );
        assert!(fired && result.is_err());
    }

    #[test]
    fn concurrent_walks_have_independent_cursors() {
        let (temp, parent) = fixture();
        for number in 0..700 {
            fs::write(temp.path().join(format!("parent/file-{number:04}")), []).unwrap();
        }
        let barrier = Arc::new(std::sync::Barrier::new(3));
        let threads = (0..2)
            .map(|_| {
                let parent = parent.clone();
                let barrier = barrier.clone();
                std::thread::spawn(move || {
                    barrier.wait();
                    parent.entries(700).unwrap().unwrap()
                })
            })
            .collect::<Vec<_>>();
        barrier.wait();
        let mut results = threads.into_iter().map(|thread| thread.join().unwrap());
        let first = results.next().unwrap();
        assert_eq!(first.len(), 700);
        assert_eq!(results.next().unwrap(), first);
    }

    #[cfg(unix)]
    #[test]
    fn nonfiles_are_listed_without_following_or_opening_content() {
        use std::os::unix::fs::symlink;
        let (temp, parent) = fixture();
        let dir = temp.path().join("parent");
        symlink("absent", dir.join("link")).unwrap();
        nix::unistd::mkfifo(&dir.join("pipe"), nix::sys::stat::Mode::S_IRUSR).unwrap();
        let entries = parent.entries(2).unwrap().unwrap();
        assert_eq!(entries[0].kind, JournalEntryKind::Symlink);
        assert_eq!(entries[1].kind, JournalEntryKind::Fifo);
        assert!(parent.read_prefix(OsStr::new("link"), 1).is_err());
        assert!(parent.read_prefix(OsStr::new("pipe"), 1).is_err());
        assert!(parent.open_child(OsStr::new("link")).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn short_read_after_observation_refuses() {
        let (temp, parent) = fixture();
        let path = temp.path().join("parent/record");
        fs::write(&path, b"abc").unwrap();
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::BeforePrefixRead,
            move || fs::write(path, b"a"),
            || parent.read_prefix(OsStr::new("record"), 3),
        );
        assert!(fired && result.is_err());
    }

    #[cfg(unix)]
    #[test]
    fn changed_file_and_rebound_parent_after_read_refuse() {
        for mutation in ["content", "leaf", "parent"] {
            let (temp, parent) = fixture();
            let dir = temp.path().join("parent");
            fs::write(dir.join("record"), b"abc").unwrap();
            let moved = dir.with_extension("moved");
            let (result, fired) = run_with_media_hook(
                MediaPrimitive::AfterPrefixRead,
                move || {
                    match mutation {
                        "content" => fs::write(dir.join("record"), b"xyz")?,
                        "leaf" => {
                            fs::rename(dir.join("record"), dir.join("old"))?;
                            fs::write(dir.join("record"), b"abc")?;
                        }
                        _ => {
                            fs::rename(&dir, moved)?;
                            fs::create_dir(&dir)?;
                            fs::write(dir.join("record"), b"abc")?;
                        }
                    }
                    Ok(())
                },
                || parent.read_prefix(OsStr::new("record"), 3),
            );
            assert!(fired && result.is_err(), "{mutation}");
        }
    }

    #[cfg(windows)]
    #[test]
    fn observed_read_excludes_mutation_then_releases_it() {
        let (temp, parent) = fixture();
        let path = temp.path().join("parent/record");
        fs::write(&path, b"abc").unwrap();
        let current = path.clone();
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::AfterPrefixRead,
            move || {
                assert!(fs::write(&current, b"xyz").is_err());
                assert!(fs::rename(&current, current.with_extension("moved")).is_err());
                Ok(())
            },
            || parent.read_prefix(OsStr::new("record"), 3),
        );
        assert!(fired && result.unwrap().unwrap().complete);
        fs::write(&path, b"xyz").unwrap();
        fs::rename(&path, path.with_extension("moved")).unwrap();
    }
}

#[cfg(unix)]
#[cfg(all(test, feature = "full-tests"))]
mod native {
    use super::*;
    use std::fs;
    use std::os::unix::fs::symlink;

    struct Fixture {
        activity: Arc<crate::activity::ActivityLease>,
        temp: tempfile::TempDir,
        parent: Arc<MediaDirectory>,
        destination: Arc<MediaDirectory>,
        digest: MediaDigest,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir_all(temp.path().join("day/segment")).unwrap();
            fs::write(temp.path().join("day/input"), b"source media").unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
            let parent = MediaDirectory::open(root.clone(), Path::new("day")).unwrap();
            let destination = MediaDirectory::open(root, Path::new("day/segment")).unwrap();
            let digest = digest_reader(&mut &b"source media"[..]).unwrap();
            Self {
                activity,
                temp,
                parent,
                destination,
                digest,
            }
        }
        fn target(&self) -> OwnedMediaTarget {
            MediaStage::create(self.parent.clone(), self.activity.clone())
                .unwrap()
                .create_target()
                .unwrap()
        }
        fn prepared(&self) -> PreparedMedia {
            self.target()
                .copy_from(&self.parent, OsStr::new("input"), &self.digest)
                .unwrap()
        }
    }

    #[test]
    fn publish_confirmed_then_identical_and_conflicting_content() {
        let f = Fixture::new();
        assert!(f.parent.available_bytes().unwrap() > 0);
        let (capacity, fired) = run_with_media_hook(
            MediaPrimitive::CapacityQuery,
            || Err(io::Error::other("query failed")),
            || f.parent.available_bytes(),
        );
        assert!(fired && capacity.is_err());
        let mut prepared = f.prepared();
        let identity = prepared.identity();
        let r = prepared.publish(&f.destination, OsStr::new("audio.flac"));
        assert_eq!(r.effect, PublicationEffect::Published);
        assert_eq!(r.confirmation, Confirmation::Confirmed);
        assert_eq!(r.cleanup, Cleanup::Complete);
        assert_eq!(r.parent_durability, Durability::Synced);
        assert_eq!(r.identity, Some(identity));
        let mut identical = f.prepared();
        assert_ne!(identical.identity(), identity);
        let r = identical.publish(&f.destination, OsStr::new("audio.flac"));
        assert_eq!(r.effect, PublicationEffect::AlreadyPresent);
        assert_eq!(r.identity, Some(identity));
        fs::write(f.temp.path().join("day/segment/conflict"), b"competitor").unwrap();
        let r = f.prepared().publish(&f.destination, OsStr::new("conflict"));
        assert_eq!(r.effect, PublicationEffect::Conflict);
        assert_eq!(
            fs::read(f.temp.path().join("day/segment/conflict")).unwrap(),
            b"competitor"
        );
    }

    #[test]
    fn occupied_kinds_and_absent_or_rebound_destination_are_preserved() {
        let f = Fixture::new();
        fs::create_dir(f.temp.path().join("day/segment/directory")).unwrap();
        symlink("../input", f.temp.path().join("day/segment/symlink")).unwrap();
        for name in ["directory", "symlink"] {
            assert_eq!(
                f.prepared()
                    .publish(&f.destination, OsStr::new(name))
                    .effect,
                PublicationEffect::Conflict
            );
        }
        let mut prepared = f.prepared();
        fs::rename(
            f.temp.path().join("day/segment"),
            f.temp.path().join("day/moved"),
        )
        .unwrap();
        assert_eq!(
            prepared.publish(&f.destination, OsStr::new("new")).effect,
            PublicationEffect::NotPublished
        );
        assert!(!f.temp.path().join("day/segment").exists());
        fs::create_dir(f.temp.path().join("day/segment")).unwrap();
        assert_eq!(
            prepared.publish(&f.destination, OsStr::new("new")).effect,
            PublicationEffect::NotPublished
        );
        assert!(!f.temp.path().join("day/segment/new").exists());
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
    }

    #[test]
    fn mismatch_and_replaced_stage_keep_cleanup_ownership() {
        let f = Fixture::new();
        for digest in [
            MediaDigest {
                size: 1,
                ..f.digest.clone()
            },
            MediaDigest {
                sha256: "0".repeat(64),
                ..f.digest.clone()
            },
        ] {
            let mut failed = f
                .target()
                .copy_from(&f.parent, OsStr::new("input"), &digest)
                .unwrap_err()
                .owned;
            assert_eq!(failed.cleanup_after_helper_quiescence(), Cleanup::Complete);
        }
        let target = f.target();
        let path = target.stage.diagnostic_path();
        fs::rename(&path, path.with_extension("moved")).unwrap();
        fs::create_dir(&path).unwrap();
        fs::write(path.join("payload"), b"replacement").unwrap();
        let mut failed = target
            .copy_from(&f.parent, OsStr::new("input"), &f.digest)
            .unwrap_err()
            .owned;
        assert_eq!(failed.cleanup_after_helper_quiescence(), Cleanup::Refused);
        assert_eq!(fs::read(path.join("payload")).unwrap(), b"replacement");
    }

    #[test]
    fn fresh_descriptor_transport_is_one_shot_and_not_path_retargeted() {
        use std::os::fd::AsRawFd;
        let f = Fixture::new();
        let mut target = f.target();
        let path = target.stage.diagnostic_path().join("payload");
        assert!(!target.helper_transport_issued());
        let transport = target.helper_transport().unwrap();
        #[cfg(target_os = "linux")]
        assert_eq!(
            transport.target,
            PathBuf::from(format!(
                "/proc/self/fd/{}",
                transport.descriptor.as_raw_fd()
            ))
        );
        #[cfg(not(target_os = "linux"))]
        assert_eq!(
            transport.target,
            PathBuf::from(format!("/dev/fd/{}", transport.descriptor.as_raw_fd()))
        );
        let owned = transport.descriptor.try_clone_to_owned().unwrap();
        drop(transport);
        assert!(target.helper_transport_issued());
        assert!(target.helper_transport().is_err());
        fs::rename(&path, path.with_extension("moved")).unwrap();
        fs::write(&path, b"competitor").unwrap();
        let mut file = File::from(owned);
        assert_eq!(file.stream_position().unwrap(), 0);
        assert_eq!(file.metadata().unwrap().len(), 0);
        file.write_all(b"source media").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"competitor");
        assert!(target.prepare_after_successful_helper(&f.digest).is_err());
    }

    #[test]
    fn prepublication_faults_retain_source_and_cleanup_authority() {
        let f = Fixture::new();
        for primitive in [
            MediaPrimitive::PreparationRead,
            MediaPrimitive::PreparationWrite,
            MediaPrimitive::FileSync,
        ] {
            let (result, fired) = run_with_media_hook(
                primitive,
                || Err(io::Error::other("fault")),
                || {
                    f.target()
                        .copy_from(&f.parent, OsStr::new("input"), &f.digest)
                },
            );
            assert!(fired);
            let mut failed = result.unwrap_err().owned;
            assert_eq!(failed.cleanup_after_helper_quiescence(), Cleanup::Complete);
        }
        let mut prepared = f.prepared();
        let (receipt, fired) = run_with_media_hook(
            MediaPrimitive::BeforePublish,
            || Err(io::Error::other("fault")),
            || prepared.publish(&f.destination, OsStr::new("new")),
        );
        assert!(fired);
        assert_eq!(receipt.effect, PublicationEffect::NotPublished);
        assert!(!f.temp.path().join("day/segment/new").exists());
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
    }

    #[test]
    fn postpublication_faults_preserve_effect_and_separate_obligations() {
        for primitive in [
            MediaPrimitive::AfterPublish,
            MediaPrimitive::ParentSync,
            MediaPrimitive::StageUnlink,
            MediaPrimitive::Completion,
        ] {
            let f = Fixture::new();
            let mut prepared = f.prepared();
            let (receipt, fired) = run_with_media_hook(
                primitive,
                || Err(io::Error::other("fault")),
                || prepared.publish(&f.destination, OsStr::new("new")),
            );
            assert!(fired);
            assert_eq!(receipt.effect, PublicationEffect::Published);
            assert_eq!(
                fs::read(f.temp.path().join("day/segment/new")).unwrap(),
                b"source media"
            );
            match primitive {
                MediaPrimitive::ParentSync => {
                    assert_eq!(receipt.parent_durability, Durability::Unproven);
                    assert_eq!(receipt.confirmation, Confirmation::Confirmed);
                }
                MediaPrimitive::StageUnlink => assert_eq!(receipt.cleanup, Cleanup::Pending),
                _ => assert_eq!(receipt.confirmation, Confirmation::Unverified),
            }
            assert_eq!(prepared.cleanup(), Cleanup::Complete);
        }
    }

    #[test]
    fn postpublication_rebinding_never_rolls_back_or_confirms_replacement() {
        for primitive in [MediaPrimitive::AfterPublish, MediaPrimitive::Completion] {
            let f = Fixture::new();
            let mut prepared = f.prepared();
            let segment = f.temp.path().join("day/segment");
            let moved = f.temp.path().join("day/moved");
            let replacement = segment.clone();
            let (receipt, fired) = run_with_media_hook(
                primitive,
                move || {
                    fs::rename(&segment, moved)?;
                    fs::create_dir(&replacement)?;
                    fs::write(replacement.join("new"), b"competitor")
                },
                || prepared.publish(&f.destination, OsStr::new("new")),
            );
            assert!(fired);
            assert_eq!(receipt.effect, PublicationEffect::Published);
            assert_eq!(receipt.confirmation, Confirmation::Unverified);
            assert_eq!(
                fs::read(f.temp.path().join("day/segment/new")).unwrap(),
                b"competitor"
            );
            assert_eq!(
                fs::read(f.temp.path().join("day/moved/new")).unwrap(),
                b"source media"
            );
            assert_eq!(prepared.cleanup(), Cleanup::Complete);
        }
    }

    #[test]
    fn prepared_source_change_and_replaced_leaf_refuse_before_mutation() {
        let f = Fixture::new();
        let mut prepared = f.prepared();
        let path = prepared.target.stage.diagnostic_path().join("payload");
        fs::write(&path, b"changed data").unwrap();
        assert_eq!(
            prepared.publish(&f.destination, OsStr::new("new")).effect,
            PublicationEffect::NotPublished
        );
        fs::rename(&path, path.with_extension("moved")).unwrap();
        fs::write(&path, b"competitor").unwrap();
        assert_eq!(prepared.cleanup(), Cleanup::Refused);
        assert_eq!(fs::read(path).unwrap(), b"competitor");
    }

    #[cfg(target_os = "linux")]
    #[ignore = "requires a separate /dev/shm filesystem"]
    #[test]
    fn cross_volume_refusal_never_copies() {
        let f = Fixture::new();
        let other = tempfile::tempdir_in("/dev/shm").unwrap();
        let root = Arc::new(JournalRoot::open(other.path()).unwrap());
        let destination = MediaDirectory::open(root, Path::new("")).unwrap();
        assert!(
            !destination.identity().same_volume(f.parent.identity()),
            "fixture must span volumes"
        );
        let mut prepared = f.prepared();
        let receipt = prepared.publish(&destination, OsStr::new("new"));
        assert_eq!(receipt.effect, PublicationEffect::NotPublished);
        assert!(!other.path().join("new").exists());
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
    }

    #[test]
    fn modification_at_file_sync_cannot_become_prepared() {
        let f = Fixture::new();
        let target = f.target();
        let path = target.stage.diagnostic_path().join("payload");
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::FileSync,
            move || fs::write(&path, b"changed data"),
            || target.copy_from(&f.parent, OsStr::new("input"), &f.digest),
        );
        assert!(fired);
        let mut failure = result.unwrap_err().owned;
        assert_eq!(failure.cleanup_after_helper_quiescence(), Cleanup::Complete);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod archive_input {
    use super::*;
    use std::fs;
    use std::process::Command;

    fn fixture() -> tempfile::TempDir {
        match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
            Some(root) => tempfile::tempdir_in(root).unwrap(),
            None => tempfile::tempdir().unwrap(),
        }
    }

    #[test]
    #[ignore = "requires pinned native restic and a disposable repository"]
    fn named_input_is_guarded_and_keeps_original_basename_through_real_archive() {
        let temp = fixture();
        fs::create_dir_all(temp.path().join("day/segment")).unwrap();
        fs::write(
            temp.path().join("day/input"),
            b"archive original basename\n",
        )
        .unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
        let parent = MediaDirectory::open(root.clone(), Path::new("day")).unwrap();
        let destination = MediaDirectory::open(root, Path::new("day/segment")).unwrap();
        let expected = MediaDigest {
            size: 26,
            sha256: "75120243cd13919b80d6f64ae1b1fa974cdf99b8c7d95c7f02d3e5d943d182d3".into(),
        };
        let mut prepared = MediaStage::create(parent.clone(), activity.clone())
            .unwrap()
            .create_named_target(OsStr::new("audio.flac"))
            .unwrap()
            .copy_from(&parent, OsStr::new("input"), &expected)
            .unwrap();
        let identity = prepared.identity();
        let input = prepared.archive_source().unwrap();
        assert_eq!(input.name(), "audio.flac");
        assert_eq!(input.path().file_name(), Some(OsStr::new("audio.flac")));
        assert_eq!(input.identity(), identity);
        assert_eq!(input.digest(), &expected);
        input.revalidate().unwrap();
        let repo = temp.path().join("repo");
        let restic = std::env::var_os("SOLSTONE_NATIVE_RESTIC").unwrap_or_else(|| "restic".into());
        let command = || {
            let mut c = Command::new(&restic);
            c.env("RESTIC_PASSWORD", "synthetic-native-test")
                .arg("--repo")
                .arg(&repo);
            c
        };
        let version = Command::new(&restic).arg("version").output().unwrap();
        assert!(version.status.success());
        let version = String::from_utf8(version.stdout).unwrap();
        assert!(version.starts_with("restic 0.19.0 "), "{version}");
        println!("NATIVE_ARCHIVE_TOOL {}", version.trim());
        let init = command().arg("init").output().unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let mut backup = command();
        backup
            .arg("backup")
            .arg("--force")
            .arg("--json")
            .arg(input.path());
        #[cfg(target_os = "linux")]
        {
            use std::os::fd::AsRawFd;
            use std::os::unix::process::CommandExt;
            let fd = input.descriptor().unwrap().as_raw_fd();
            // SAFETY: child-only fcntl, no allocation/locks in pre_exec; the
            // borrowed directory remains retained through wait_with_output.
            #[allow(unsafe_code)]
            unsafe {
                backup.pre_exec(move || {
                    nix::libc::fcntl(fd, nix::libc::F_SETFD, 0)
                        .eq(&0)
                        .then_some(())
                        .ok_or_else(io::Error::last_os_error)
                });
            }
        }
        let output = backup.output().unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        input.revalidate().unwrap();
        let summary: serde_json::Value = String::from_utf8(output.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .find(|value| value["message_type"] == "summary")
            .unwrap();
        let snapshot = summary["snapshot_id"].as_str().unwrap();
        assert_eq!(snapshot.len(), 64);
        assert!(snapshot.bytes().all(|byte| byte.is_ascii_hexdigit()));
        // Query the actual full snapshot and exact entry; no suffix/name guess
        // is used as readback authority.
        let listing = command()
            .arg("ls")
            .arg("--json")
            .arg(snapshot)
            .output()
            .unwrap();
        assert!(listing.status.success());
        let entries: Vec<serde_json::Value> = String::from_utf8(listing.stdout)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|value| value["type"] == "file")
            .collect();
        assert_eq!(entries.len(), 1);
        assert_eq!(entries[0]["name"], "audio.flac");
        let archived_path = entries[0]["path"].as_str().unwrap();
        let dump = command()
            .arg("dump")
            .arg(snapshot)
            .arg(archived_path)
            .output()
            .unwrap();
        assert!(dump.status.success());
        assert_eq!(dump.stdout, b"archive original basename\n");
        assert_eq!(
            digest_reader(&mut dump.stdout.as_slice()).unwrap(),
            expected
        );
        println!(
            "NATIVE_ARCHIVE_READBACK snapshot={snapshot} path={archived_path} identity={:?} digest={expected:?}",
            input.identity()
        );
        input.revalidate().unwrap();
        drop(input);
        let receipt = prepared.publish(&destination, OsStr::new("audio.flac"));
        assert_eq!(receipt.effect, PublicationEffect::Published);
        assert_eq!(receipt.confirmation, Confirmation::Confirmed);
        assert_eq!(receipt.cleanup, Cleanup::Complete);
    }

    #[test]
    fn invalid_names_never_create_leaf_and_source_substitution_refuses() {
        let temp = fixture();
        fs::create_dir(temp.path().join("day")).unwrap();
        fs::write(temp.path().join("day/input"), b"abc").unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
        let root_directory = MediaDirectory::open(root.clone(), Path::new("")).unwrap();
        assert_eq!(root_directory.object_identity(), root.identity());
        let parent = MediaDirectory::open(root.clone(), Path::new("day")).unwrap();
        for invalid in ["", "..", "../audio.flac", "CON", "x:y", "x\\y"] {
            let mut failed = MediaStage::create(parent.clone(), activity.clone())
                .unwrap()
                .create_named_target(OsStr::new(invalid))
                .err()
                .unwrap();
            assert!(failed.owned.leaf.is_none());
            assert_eq!(
                fs::read_dir(failed.owned.diagnostic_path())
                    .unwrap()
                    .count(),
                0
            );
            assert_eq!(failed.owned.cleanup(), Cleanup::Complete);
        }
        let expected = MediaDigest {
            size: 3,
            sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad".into(),
        };
        let target = MediaStage::create(parent.clone(), activity.clone())
            .unwrap()
            .create_named_target(OsStr::new("audio.flac"))
            .unwrap();
        assert!(Arc::ptr_eq(target.journal_root(), &root));
        let mut prepared = target
            .copy_from(&parent, OsStr::new("input"), &expected)
            .unwrap();
        let path = prepared.target.stage.diagnostic_path().join("audio.flac");
        let source = prepared.archive_source().unwrap();
        #[cfg(unix)]
        {
            fs::write(&path, b"xyz").unwrap();
            assert!(source.revalidate().is_err());
            drop(source);
            assert!(prepared.archive_source().is_err());
            // Retained source pathname and ancestor substitution are refused.
            let stage = prepared.target.stage.diagnostic_path();
            fs::rename(&stage, stage.with_extension("moved")).unwrap();
            fs::create_dir(&stage).unwrap();
            fs::write(stage.join("audio.flac"), b"abc").unwrap();
            assert!(prepared.archive_source().is_err());
            assert_eq!(fs::read(stage.join("audio.flac")).unwrap(), b"abc");
        }
        #[cfg(windows)]
        {
            assert!(fs::write(&path, b"xyz").is_err());
            assert!(fs::rename(&path, path.with_extension("moved")).is_err());
            source.revalidate().unwrap();
            drop(source);
            drop(prepared);
            fs::write(&path, b"xyz").unwrap();
        }
    }
}

#[cfg(windows)]
#[cfg(all(test, feature = "full-tests"))]
mod windows_native {
    use super::*;
    use std::fs::{self, OpenOptions};
    use std::os::windows::fs::{OpenOptionsExt, symlink_file};
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };

    struct Fixture {
        activity: Arc<crate::activity::ActivityLease>,
        parent: Arc<MediaDirectory>,
        destination: Arc<MediaDirectory>,
        digest: MediaDigest,
        temp: tempfile::TempDir,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
                Some(path) => tempfile::tempdir_in(path).unwrap(),
                None => tempfile::tempdir().unwrap(),
            };
            fs::create_dir_all(temp.path().join("day/segment")).unwrap();
            fs::write(temp.path().join("day/input"), b"source media").unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
            let parent = MediaDirectory::open(root.clone(), Path::new("day")).unwrap();
            let destination = MediaDirectory::open(root, Path::new("day/segment")).unwrap();
            let digest = digest_reader(&mut &b"source media"[..]).unwrap();
            Self {
                activity,
                parent,
                destination,
                digest,
                temp,
            }
        }
        fn target(&self) -> OwnedMediaTarget {
            MediaStage::create(self.parent.clone(), self.activity.clone())
                .unwrap()
                .create_target()
                .unwrap()
        }
        fn prepared(&self) -> PreparedMedia {
            self.target()
                .copy_from(&self.parent, OsStr::new("input"), &self.digest)
                .unwrap()
        }
        fn live(&self, name: &str) -> PathBuf {
            self.temp.path().join("day/segment").join(name)
        }
    }
    fn assert_shared(error: io::Error) {
        assert!(matches!(error.raw_os_error(), Some(5 | 32)), "{error:?}");
    }

    #[test]
    fn local_publication_confirms_full_identity_and_distinct_already_present() {
        let f = Fixture::new();
        let target = f.target();
        assert!(target.available_bytes().unwrap() > 0);
        let (capacity, fired) = run_with_media_hook(
            MediaPrimitive::CapacityQuery,
            || Err(io::Error::other("query failed")),
            || target.available_bytes(),
        );
        assert!(fired && capacity.is_err());
        let mut prepared = target
            .copy_from(&f.parent, OsStr::new("input"), &f.digest)
            .unwrap();
        let id = prepared.identity();
        let stage = prepared.target.stage.diagnostic_path();
        let r = prepared.publish(&f.destination, OsStr::new("audio.flac"));
        assert_eq!(r.effect, PublicationEffect::Published);
        assert_eq!(r.confirmation, Confirmation::Confirmed);
        assert_eq!(r.file_durability, Durability::Synced);
        assert_eq!(r.parent_durability, Durability::Unproven);
        assert_eq!(r.cleanup, Cleanup::Complete);
        assert_eq!(r.identity, Some(id));
        assert!(!stage.exists());
        assert_eq!(
            f.destination
                .observe_file(OsStr::new("audio.flac"))
                .unwrap()
                .unwrap()
                .identity,
            id
        );
        eprintln!("WINDOWS_MEDIA_ID source={id:?} final={:?}", r.identity);
        assert_shared(fs::write(f.live("audio.flac"), b"competitor").unwrap_err());
        assert_shared(fs::remove_file(f.live("audio.flac")).unwrap_err());
        let mut identical = f.prepared();
        assert_ne!(identical.identity(), id);
        let r = identical.publish(&f.destination, OsStr::new("audio.flac"));
        assert_eq!(r.effect, PublicationEffect::AlreadyPresent);
        assert_eq!(r.confirmation, Confirmation::Confirmed);
        assert_eq!(r.identity, Some(id));
        assert_eq!(r.cleanup, Cleanup::Complete);
        drop(identical);
        drop(prepared);
        fs::write(f.live("audio.flac"), b"competitor").unwrap(); // inverse write
        fs::remove_file(f.live("audio.flac")).unwrap(); // inverse delete
    }

    #[test]
    fn occupied_plain_directory_and_reparse_preserve_competitors() {
        let f = Fixture::new();
        fs::write(f.live("different"), b"competitor").unwrap();
        fs::create_dir(f.live("directory")).unwrap();
        symlink_file(f.temp.path().join("day/input"), f.live("symlink")).unwrap();
        for name in ["different", "directory", "symlink"] {
            let mut prepared = f.prepared();
            let r = prepared.publish(&f.destination, OsStr::new(name));
            assert_eq!(r.effect, PublicationEffect::Conflict, "{name}: {r:?}");
            assert_eq!(prepared.cleanup(), Cleanup::Complete);
        }
        assert_eq!(fs::read(f.live("different")).unwrap(), b"competitor");
        assert!(
            fs::symlink_metadata(f.live("symlink"))
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert!(f.live("directory").is_dir());
    }

    #[test]
    fn retained_source_and_ancestors_exclude_mutation_with_inverse_controls() {
        let f = Fixture::new();
        let input = f.temp.path().join("day/input");
        let input_check = input.clone();
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::PreparationWrite,
            move || {
                assert_shared(fs::write(&input_check, b"competitor").unwrap_err());
                assert_shared(fs::remove_file(&input_check).unwrap_err());
                assert_shared(
                    fs::rename(&input_check, input_check.with_extension("moved")).unwrap_err(),
                );
                Ok(())
            },
            || {
                f.target()
                    .copy_from(&f.parent, OsStr::new("input"), &f.digest)
            },
        );
        assert!(fired);
        let prepared = result.unwrap();
        fs::write(&input, b"competitor").unwrap();
        fs::rename(&input, input.with_extension("moved")).unwrap();
        fs::remove_file(input.with_extension("moved")).unwrap();
        let stage = prepared.target.stage.diagnostic_path();
        let payload = stage.join("payload");
        assert_shared(fs::write(&payload, b"competitor").unwrap_err());
        assert_shared(fs::rename(&payload, payload.with_extension("moved")).unwrap_err());
        for path in [
            f.temp.path().to_path_buf(),
            f.temp.path().join("day"),
            f.live(""),
            stage.clone(),
        ] {
            assert_shared(fs::rename(&path, path.with_extension("moved")).unwrap_err());
        }
        // Drop closes ownership without unlinking. The exact previously refused
        // payload write/rename/delete and stage rename now succeed.
        drop(prepared);
        fs::write(&payload, b"competitor").unwrap();
        fs::rename(&payload, payload.with_extension("moved")).unwrap();
        fs::remove_file(payload.with_extension("moved")).unwrap();
        fs::rename(&stage, stage.with_extension("moved")).unwrap();
        fs::remove_dir(stage.with_extension("moved")).unwrap();
        let stage = MediaStage::create(f.parent.clone(), f.activity.clone()).unwrap();
        let stage_path = stage.diagnostic_path();
        drop(stage);
        fs::rename(&stage_path, stage_path.with_extension("moved")).unwrap();
        fs::remove_dir(stage_path.with_extension("moved")).unwrap();
        let root_path = f.temp.path().to_path_buf();
        drop(f.destination);
        drop(f.parent);
        drop(f.activity);
        fs::rename(root_path.join("day/segment"), root_path.join("day/moved")).unwrap();
        fs::rename(root_path.join("day"), root_path.join("moved")).unwrap();
        let renamed = root_path.with_extension("moved");
        fs::rename(&root_path, &renamed).unwrap();
        fs::rename(renamed, root_path).unwrap();
    }

    #[test]
    fn archive_access_late_failures_retain_actual_object_in_both_directions() {
        let f = Fixture::new();
        let mut prepared = f.prepared();
        let path = prepared.target.stage.diagnostic_path().join("payload");
        let before = prepared.identity();
        let (result, fired) = run_with_media_hook(
            MediaPrimitive::ArchiveAccessGap,
            || Err(io::Error::other("final read admission failed")),
            || prepared.archive_source().map(|_| ()),
        );
        assert!(fired && result.is_err());
        assert_eq!(prepared.identity(), before);
        assert_eq!(fs::read(&path).unwrap(), b"source media");
        let source = prepared.archive_source().unwrap();
        assert_shared(fs::write(&path, b"competitor").unwrap_err());
        source.revalidate().unwrap();
        drop(source);
        let (receipt, fired) = run_with_media_hook(
            MediaPrimitive::ArchiveAccessGap,
            || Err(io::Error::other("final mutation admission failed")),
            || prepared.publish(&f.destination, OsStr::new("audio.flac")),
        );
        assert!(fired);
        assert_eq!(receipt.effect, PublicationEffect::NotPublished);
        assert!(receipt.error.is_some());
        assert_eq!(prepared.identity(), before);
        assert_eq!(fs::read(&path).unwrap(), b"source media");
        assert!(!f.live("audio.flac").exists());
        let receipt = prepared.publish(&f.destination, OsStr::new("audio.flac"));
        assert_eq!(receipt.effect, PublicationEffect::Published);
        assert_eq!(receipt.confirmation, Confirmation::Confirmed);
        assert_eq!(receipt.identity, Some(before));
    }

    #[test]
    fn archive_transition_needs_fresh_flush_before_publication() {
        let f = Fixture::new();
        let mut prepared = f.prepared();
        prepared.archive_source().unwrap().revalidate().unwrap();
        let (receipt, fired) = run_with_media_hook(
            MediaPrimitive::FileSync,
            || Err(io::Error::other("post-transition flush failed")),
            || prepared.publish(&f.destination, OsStr::new("audio.flac")),
        );
        assert!(fired);
        assert_eq!(receipt.effect, PublicationEffect::NotPublished);
        assert_eq!(receipt.file_durability, Durability::Unproven);
        assert!(receipt.error.is_some());
        assert!(!f.live("audio.flac").exists());
        let receipt = prepared.publish(&f.destination, OsStr::new("audio.flac"));
        assert_eq!(receipt.effect, PublicationEffect::Published);
        assert_eq!(receipt.file_durability, Durability::Synced);
    }

    #[test]
    fn archive_access_gap_content_and_name_substitution_never_admit_competitor() {
        for (replace, mutation) in [(false, false), (true, false), (false, true), (true, true)] {
            let f = Fixture::new();
            let mut prepared = f.prepared();
            if mutation {
                prepared.archive_source().unwrap().revalidate().unwrap();
            }
            let identity = prepared.identity();
            let path = prepared.target.stage.diagnostic_path().join("payload");
            let altered = path.clone();
            let (result, fired) = run_with_media_hook(
                MediaPrimitive::ArchiveAccessGap,
                move || {
                    if replace {
                        fs::rename(&altered, altered.with_extension("moved"))?;
                    }
                    // Same size; in-place mode retains the exact native ID.
                    fs::write(&altered, b"newer bytes!")
                },
                || {
                    if mutation {
                        let receipt = prepared.publish(&f.destination, OsStr::new("audio.flac"));
                        assert_eq!(receipt.effect, PublicationEffect::NotPublished);
                        Err(receipt.error.expect("changed source refuses publication"))
                    } else {
                        prepared.archive_source().map(|_| ())
                    }
                },
            );
            assert!(fired && result.is_err());
            assert_eq!(prepared.identity(), identity);
            assert_eq!(fs::read(&path).unwrap(), b"newer bytes!");
            assert!(!f.live("audio.flac").exists());
            assert_eq!(
                prepared
                    .publish(&f.destination, OsStr::new("audio.flac"))
                    .effect,
                PublicationEffect::NotPublished
            );
            assert!(prepared.archive_source().is_err());
            if replace {
                assert_eq!(prepared.cleanup(), Cleanup::Refused);
                assert_eq!(
                    fs::read(path.with_extension("moved")).unwrap(),
                    b"source media"
                );
                assert_eq!(fs::read(&path).unwrap(), b"newer bytes!");
            }
            drop(prepared);
            assert_eq!(fs::read(&path).unwrap(), b"newer bytes!");
        }
    }

    #[test]
    fn missing_destination_and_wrong_digest_never_publish_or_enable_helper() {
        let f = Fixture::new();
        let root = Arc::new(JournalRoot::open(f.temp.path()).unwrap());
        assert!(MediaDirectory::open(root, Path::new("day/missing")).is_err());
        assert!(!f.temp.path().join("day/missing").exists());
        for digest in [
            MediaDigest {
                size: 1,
                ..f.digest.clone()
            },
            MediaDigest {
                sha256: "0".repeat(64),
                ..f.digest.clone()
            },
        ] {
            let mut failure = f
                .target()
                .copy_from(&f.parent, OsStr::new("input"), &digest)
                .unwrap_err()
                .owned;
            assert_eq!(failure.cleanup_after_helper_quiescence(), Cleanup::Complete);
        }
        let mut target = f.target();
        assert_eq!(
            target.helper_transport().unwrap_err().kind(),
            io::ErrorKind::Unsupported
        );
        assert!(!target.helper_taken);
        let leaf = target.stage.leaf.as_mut().unwrap();
        assert_eq!(leaf.file.metadata().unwrap().len(), 0);
        assert_eq!(leaf.file.stream_position().unwrap(), 0);
        assert_eq!(target.cleanup_after_helper_quiescence(), Cleanup::Complete);
    }

    #[test]
    fn native_faults_preserve_effect_and_never_dispose_live_renamed_handle() {
        let f = Fixture::new();
        for primitive in [
            MediaPrimitive::PreparationRead,
            MediaPrimitive::PreparationWrite,
            MediaPrimitive::FileSync,
        ] {
            let (result, fired) = run_with_media_hook(
                primitive,
                || Err(io::Error::other("fault")),
                || {
                    f.target()
                        .copy_from(&f.parent, OsStr::new("input"), &f.digest)
                },
            );
            assert!(fired);
            let mut failure = result.unwrap_err().owned;
            assert_eq!(failure.cleanup_after_helper_quiescence(), Cleanup::Complete);
            assert!(!f.live("new").exists());
        }
        for primitive in [
            MediaPrimitive::BeforePublish,
            MediaPrimitive::AfterPublish,
            MediaPrimitive::ParentSync,
            MediaPrimitive::StageRemove,
            MediaPrimitive::Completion,
        ] {
            let mut prepared = f.prepared();
            let name = format!("{primitive:?}");
            let (r, fired) = run_with_media_hook(
                primitive,
                || Err(io::Error::other("fault")),
                || prepared.publish(&f.destination, OsStr::new(&name)),
            );
            assert!(fired);
            if primitive == MediaPrimitive::BeforePublish {
                assert_eq!(r.effect, PublicationEffect::NotPublished);
                assert!(!f.live(&name).exists());
            } else {
                assert_eq!(r.effect, PublicationEffect::Published);
                assert_eq!(fs::read(f.live(&name)).unwrap(), b"source media");
                assert_shared(fs::remove_file(f.live(&name)).unwrap_err());
            }
            assert_eq!(prepared.cleanup(), Cleanup::Complete);
            if primitive != MediaPrimitive::BeforePublish {
                assert_eq!(fs::read(f.live(&name)).unwrap(), b"source media");
            }
        }
    }

    #[test]
    fn cleanup_waits_for_physical_absence_and_preserves_nonempty_stage() {
        let f = Fixture::new();
        let mut target = f.target();
        let stage = target.stage.diagnostic_path();
        let foreign = OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(stage.join("payload"))
            .unwrap();
        assert_eq!(target.cleanup_after_helper_quiescence(), Cleanup::Pending);
        assert!(!target.stage.removed);
        drop(foreign);
        assert_eq!(target.cleanup_after_helper_quiescence(), Cleanup::Complete);
        assert!(!stage.exists());
        let mut target = f.target();
        let stage = target.stage.diagnostic_path();
        fs::write(stage.join("foreign"), b"preserve").unwrap();
        assert_eq!(target.cleanup_after_helper_quiescence(), Cleanup::Pending);
        assert_eq!(fs::read(stage.join("foreign")).unwrap(), b"preserve");
        fs::remove_file(stage.join("foreign")).unwrap();
        assert_eq!(target.cleanup_after_helper_quiescence(), Cleanup::Complete);
    }

    #[test]
    #[ignore = "requires SOLSTONE_MEDIA_OTHER_ROOT on a separate Windows volume"]
    fn cross_volume_has_no_copy_fallback() {
        let f = Fixture::new();
        let other =
            tempfile::tempdir_in(std::env::var_os("SOLSTONE_MEDIA_OTHER_ROOT").unwrap()).unwrap();
        let root = Arc::new(JournalRoot::open(other.path()).unwrap());
        let destination = MediaDirectory::open(root, Path::new("")).unwrap();
        assert!(!destination.identity().same_volume(f.parent.identity()));
        let mut prepared = f.prepared();
        let r = prepared.publish(&destination, OsStr::new("new"));
        assert_eq!(r.effect, PublicationEffect::NotPublished);
        assert!(!other.path().join("new").exists());
        assert_eq!(prepared.cleanup(), Cleanup::Complete);
        drop(prepared);
        drop(destination);
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod removal {
    use super::*;
    use std::fs;

    struct Fixture {
        activity: Arc<crate::activity::ActivityLease>,
        _temp: tempfile::TempDir,
        parent: Arc<MediaDirectory>,
        source: Arc<MediaDirectory>,
        raw: PathBuf,
        expected: MediaObservation,
    }
    impl Fixture {
        fn new() -> Self {
            let temp = match std::env::var_os("SOLSTONE_MEDIA_TEST_ROOT") {
                Some(root) => tempfile::tempdir_in(root).unwrap(),
                None => tempfile::tempdir().unwrap(),
            };
            fs::create_dir(temp.path().join("parent")).unwrap();
            fs::create_dir(temp.path().join("parent/segment")).unwrap();
            let raw = temp.path().join("parent/segment/raw.flac");
            fs::write(&raw, b"abc").unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
            let parent = MediaDirectory::open(root, Path::new("parent")).unwrap();
            let source = parent.open_child(OsStr::new("segment")).unwrap().unwrap();
            let expected = source
                .observe_file(OsStr::new("raw.flac"))
                .unwrap()
                .unwrap();
            assert_eq!(expected.digest.size, 3);
            assert_eq!(
                expected.digest.sha256,
                "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
            );
            Self {
                activity,
                _temp: temp,
                parent,
                source,
                raw,
                expected,
            }
        }
        fn stage(&self) -> MediaStage {
            MediaStage::create(Arc::clone(&self.parent), self.activity.clone()).unwrap()
        }
        fn remove(&self, stage: MediaStage) -> MediaRemoval {
            self.source
                .remove_observed(OsStr::new("raw.flac"), &self.expected, stage)
        }
    }
    fn fault() -> io::Result<()> {
        Err(io::Error::other("injected native removal cut"))
    }

    #[test]
    fn exact_observed_removal_reports_independent_facts_and_strict_stage_observation() {
        let f = Fixture::new();
        let stage = f.stage();
        let path = stage.diagnostic_path();
        let observation = stage.observation().unwrap();
        assert_eq!(observation.parent_relative, ["parent"]);
        assert_eq!(observation.parent_identity, f.parent.identity());
        let encoded = serde_json::to_value(&observation).unwrap();
        assert_eq!(
            serde_json::from_value::<StageObservation>(encoded.clone()).unwrap(),
            observation
        );
        let mut unknown = encoded;
        unknown["cleanup_authority"] = serde_json::json!(true);
        assert!(serde_json::from_value::<StageObservation>(unknown).is_err());
        let removed = f.remove(stage);
        let receipt = removed.receipt();
        assert_eq!(receipt.effect, RemovalEffect::Deleted, "{receipt:?}");
        assert_eq!(receipt.original_name, OriginalName::Absent);
        assert_eq!(receipt.claim_confirmation, Confirmation::Confirmed);
        assert_eq!(receipt.claimed_identity, Some(f.expected.identity));
        assert!(!receipt.restoration_attempted);
        assert_eq!(receipt.cleanup, Cleanup::Complete, "{receipt:?}");
        assert_eq!(removed.stage_observation(), Some(&observation));
        assert!(!f.raw.exists());
        assert!(!path.exists());
        #[cfg(unix)]
        {
            assert_eq!(receipt.file_durability, Durability::Unproven);
            assert_eq!(receipt.source_durability, Durability::Synced);
            assert_eq!(receipt.claim_durability, Durability::Synced);
            assert_eq!(receipt.stage_parent_durability, Durability::Synced);
            assert!(receipt.failures.is_empty(), "{receipt:?}");
        }
        #[cfg(windows)]
        {
            assert_eq!(receipt.file_durability, Durability::Unproven);
            assert_eq!(receipt.source_durability, Durability::Unproven);
            assert_eq!(receipt.claim_durability, Durability::Unproven);
            assert_eq!(receipt.stage_parent_durability, Durability::Unproven);
        }
    }

    #[cfg(unix)]
    #[test]
    fn consuming_removal_receipt_preserves_original_error_and_releases_only_handles() {
        let f = Fixture::new();
        let (removal, hit) = run_with_media_hook(
            MediaPrimitive::RemovalSourceSync,
            || Err(io::Error::from_raw_os_error(13)),
            || f.remove(f.stage()),
        );
        assert!(hit);
        let receipt = removal.receipt();
        assert_eq!(receipt.effect, RemovalEffect::Deleted);
        assert_eq!(receipt.original_name, OriginalName::Absent);
        assert_eq!(receipt.claim_confirmation, Confirmation::Confirmed);
        assert_eq!(receipt.source_durability, Durability::Unproven);
        assert_eq!(receipt.cleanup, Cleanup::Complete);
        assert_eq!(receipt.failures.len(), 1);
        assert_eq!(receipt.failures[0].error.raw_os_error(), Some(13));
        let before = Arc::strong_count(&f.activity);
        let receipt = removal.into_receipt();
        assert!(Arc::strong_count(&f.activity) < before);
        assert_eq!(receipt.effect, RemovalEffect::Deleted);
        assert_eq!(receipt.original_name, OriginalName::Absent);
        assert_eq!(receipt.claim_confirmation, Confirmation::Confirmed);
        assert_eq!(receipt.source_durability, Durability::Unproven);
        assert_eq!(receipt.cleanup, Cleanup::Complete);
        assert_eq!(receipt.claimed_identity, Some(f.expected.identity));
        assert_eq!(receipt.failures.len(), 1);
        assert_eq!(receipt.failures[0].phase, RemovalPhase::SourceSync);
        assert_eq!(
            receipt.failures[0].error.kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(receipt.failures[0].error.raw_os_error(), Some(13));
        assert!(!f.raw.exists());
    }

    #[test]
    fn absent_changed_content_and_distinct_identity_never_claim() {
        for case in 0..3 {
            let f = Fixture::new();
            match case {
                0 => fs::remove_file(&f.raw).unwrap(),
                1 => fs::write(&f.raw, b"xyz").unwrap(),
                _ => {
                    fs::rename(&f.raw, f.raw.with_extension("saved")).unwrap();
                    fs::write(&f.raw, b"abc").unwrap();
                }
            }
            let removed = f.remove(f.stage());
            assert_eq!(removed.receipt().effect, RemovalEffect::NotClaimed);
            assert_eq!(removed.receipt().cleanup, Cleanup::Complete);
            if case != 0 {
                assert_eq!(
                    fs::read(&f.raw).unwrap(),
                    if case == 1 { b"xyz" } else { b"abc" }
                );
            }
        }
    }

    #[test]
    fn malformed_names_nonfiles_and_wrong_stage_parent_preserve_contents() {
        let f = Fixture::new();
        fs::write(f.parent.diagnostic_path().join("sentinel"), b"outside").unwrap();
        for name in ["../sentinel", "/sentinel", "", ".", "CON", "raw\\escape"] {
            let result = f
                .source
                .remove_observed(OsStr::new(name), &f.expected, f.stage());
            assert_eq!(result.receipt().effect, RemovalEffect::NotClaimed);
        }
        fs::create_dir(f.raw.with_file_name("directory")).unwrap();
        let result = f
            .source
            .remove_observed(OsStr::new("directory"), &f.expected, f.stage());
        assert_eq!(result.receipt().effect, RemovalEffect::NotClaimed);
        fs::create_dir(f.parent.diagnostic_path().join("other")).unwrap();
        let other = f.parent.open_child(OsStr::new("other")).unwrap().unwrap();
        let result = f.remove(MediaStage::create(other, f.activity.clone()).unwrap());
        assert_eq!(result.receipt().effect, RemovalEffect::NotClaimed);
        assert_eq!(fs::read(&f.raw).unwrap(), b"abc");
        assert_eq!(
            fs::read(f.parent.diagnostic_path().join("sentinel")).unwrap(),
            b"outside"
        );
    }

    #[cfg(unix)]
    #[test]
    fn symlink_source_never_follows_or_claims_target() {
        let f = Fixture::new();
        let target = f.raw.with_file_name("target");
        fs::write(&target, b"abc").unwrap();
        fs::remove_file(&f.raw).unwrap();
        std::os::unix::fs::symlink(&target, &f.raw).unwrap();
        let result = f.remove(f.stage());
        assert_eq!(result.receipt().effect, RemovalEffect::NotClaimed);
        assert!(
            fs::symlink_metadata(&f.raw)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&target).unwrap(), b"abc");
    }

    #[test]
    fn occupied_stage_and_late_claim_occupation_never_replace() {
        for late in [false, true] {
            let f = Fixture::new();
            let stage = f.stage();
            let path = stage.diagnostic_path();
            let claim = path.join(super::REMOVAL_ORIGINAL_LEAF);
            let mut result = if late {
                let competitor = claim.clone();
                let (result, hit) = run_with_media_hook(
                    MediaPrimitive::NativeClaimCut,
                    move || fs::write(competitor, b"competitor"),
                    || f.remove(stage),
                );
                assert!(hit);
                result
            } else {
                fs::write(&claim, b"competitor").unwrap();
                f.remove(stage)
            };
            assert_eq!(
                result.receipt().effect,
                RemovalEffect::NotClaimed,
                "{:?}",
                result.receipt()
            );
            assert_eq!(result.cleanup(), Cleanup::Refused);
            drop(result);
            assert_eq!(fs::read(&claim).unwrap(), b"competitor");
            assert_eq!(fs::read(&f.raw).unwrap(), b"abc");
        }
    }

    #[cfg(unix)]
    #[test]
    fn source_swap_in_native_claim_gap_preserves_wrong_claim_and_original_object() {
        let f = Fixture::new();
        let stage = f.stage();
        let claim = stage.diagnostic_path().join(super::REMOVAL_ORIGINAL_LEAF);
        let raw = f.raw.clone();
        let old = raw.with_extension("saved");
        let saved = old.clone();
        let (mut result, hit) = run_with_media_hook(
            MediaPrimitive::NativeClaimCut,
            move || {
                fs::rename(&raw, saved)?;
                fs::write(raw, b"new")
            },
            || f.remove(stage),
        );
        assert!(hit);
        assert_eq!(result.receipt().effect, RemovalEffect::Claimed);
        assert_ne!(result.receipt().claimed_identity, Some(f.expected.identity));
        assert_eq!(
            result.receipt().claim_confirmation,
            Confirmation::Unverified
        );
        assert_eq!(result.cleanup(), Cleanup::Refused);
        drop(result);
        assert_eq!(fs::read(&claim).unwrap(), b"new");
        assert_eq!(fs::read(&old).unwrap(), b"abc");
    }

    #[cfg(unix)]
    #[test]
    fn same_identity_mutation_and_new_original_after_claim_are_preserved() {
        let f = Fixture::new();
        let stage = f.stage();
        let claim = stage.diagnostic_path().join(super::REMOVAL_ORIGINAL_LEAF);
        let changed_claim = claim.clone();
        let raw = f.raw.clone();
        let (mut result, hit) = run_with_media_hook(
            MediaPrimitive::BeforeClaimDelete,
            move || {
                fs::write(changed_claim, b"xyz")?;
                fs::write(raw, b"new")
            },
            || f.remove(stage),
        );
        assert!(hit);
        assert_eq!(result.receipt().effect, RemovalEffect::Claimed);
        assert_eq!(result.receipt().claimed_identity, Some(f.expected.identity));
        assert_eq!(
            result.receipt().claim_confirmation,
            Confirmation::Unverified
        );
        assert_eq!(result.receipt().original_name, OriginalName::Occupied);
        assert_eq!(result.cleanup(), Cleanup::Refused);
        drop(result);
        assert_eq!(fs::read(claim).unwrap(), b"xyz");
        assert_eq!(fs::read(&f.raw).unwrap(), b"new");
    }

    #[cfg(unix)]
    #[test]
    fn replaced_private_claim_is_never_deleted_by_cleanup_or_drop() {
        let f = Fixture::new();
        let stage = f.stage();
        let claim = stage.diagnostic_path().join(super::REMOVAL_ORIGINAL_LEAF);
        let saved = claim.with_extension("saved");
        let changed_claim = claim.clone();
        let old = saved.clone();
        let (mut result, hit) = run_with_media_hook(
            MediaPrimitive::BeforeClaimDelete,
            move || {
                fs::rename(&changed_claim, old)?;
                fs::write(changed_claim, b"foreign")
            },
            || f.remove(stage),
        );
        assert!(hit);
        assert_eq!(result.receipt().effect, RemovalEffect::Claimed);
        assert_eq!(result.cleanup(), Cleanup::Refused);
        drop(result);
        assert_eq!(fs::read(claim).unwrap(), b"foreign");
        assert_eq!(fs::read(saved).unwrap(), b"abc");
    }

    #[test]
    fn postclaim_faults_retain_raw_and_forbid_every_cleanup_route() {
        for cut in [
            MediaPrimitive::AfterRemovalClaim,
            MediaPrimitive::BeforeClaimRead,
            MediaPrimitive::BeforeClaimDelete,
        ] {
            let f = Fixture::new();
            let stage = f.stage();
            let claim = stage.diagnostic_path().join(super::REMOVAL_ORIGINAL_LEAF);
            let (mut result, hit) = run_with_media_hook(cut, fault, || f.remove(stage));
            assert!(hit);
            assert_eq!(result.receipt().effect, RemovalEffect::Claimed);
            assert_eq!(result.cleanup(), Cleanup::Refused);
            assert_eq!(result.cleanup(), Cleanup::Refused);
            drop(result);
            assert_eq!(fs::read(claim).unwrap(), b"abc");
            assert!(!f.raw.exists());
        }
    }

    #[test]
    fn applied_but_error_never_earns_deletion_or_empty_stage_cleanup() {
        for (cut, effect) in [
            (
                MediaPrimitive::ClaimAppliedError,
                RemovalEffect::ClaimUncertain,
            ),
            (
                MediaPrimitive::DeleteAppliedError,
                RemovalEffect::DeletionUncertain,
            ),
            (
                MediaPrimitive::AfterClaimDelete,
                RemovalEffect::DeletePending,
            ),
        ] {
            let f = Fixture::new();
            let stage = f.stage();
            let path = stage.diagnostic_path();
            let (mut result, hit) = run_with_media_hook(
                cut,
                || Err(io::Error::from(io::ErrorKind::AlreadyExists)),
                || f.remove(stage),
            );
            assert!(hit);
            assert_eq!(result.receipt().effect, effect, "{:?}", result.receipt());
            assert_eq!(
                result.receipt().claim_confirmation,
                Confirmation::Unverified
            );
            assert_eq!(result.cleanup(), Cleanup::Refused);
            drop(result);
            assert!(path.is_dir());
            assert!(!f.raw.exists());
            if effect == RemovalEffect::ClaimUncertain {
                assert_eq!(
                    fs::read(path.join(super::REMOVAL_ORIGINAL_LEAF)).unwrap(),
                    b"abc"
                );
            }
        }
    }

    #[cfg(unix)]
    #[test]
    fn directory_sync_failures_are_independent_of_known_deletion() {
        for cut in [
            MediaPrimitive::RemovalSourceSync,
            MediaPrimitive::RemovalClaimSync,
            MediaPrimitive::RemovalParentSync,
        ] {
            let f = Fixture::new();
            let (result, hit) = run_with_media_hook(cut, fault, || f.remove(f.stage()));
            assert!(hit);
            let receipt = result.receipt();
            assert_eq!(receipt.effect, RemovalEffect::Deleted);
            assert_eq!(receipt.claim_confirmation, Confirmation::Confirmed);
            assert_eq!(receipt.cleanup, Cleanup::Complete);
            assert_eq!(
                receipt.source_durability,
                if cut == MediaPrimitive::RemovalSourceSync {
                    Durability::Unproven
                } else {
                    Durability::Synced
                }
            );
            assert_eq!(
                receipt.claim_durability,
                if cut == MediaPrimitive::RemovalClaimSync {
                    Durability::Unproven
                } else {
                    Durability::Synced
                }
            );
            assert_eq!(
                receipt.stage_parent_durability,
                if cut == MediaPrimitive::RemovalParentSync {
                    Durability::Unproven
                } else {
                    Durability::Synced
                }
            );
        }
    }

    #[test]
    fn failed_empty_stage_cleanup_retries_without_redeleting() {
        let f = Fixture::new();
        let stage = f.stage();
        let path = stage.diagnostic_path();
        let (mut result, hit) =
            run_with_media_hook(MediaPrimitive::StageRemove, fault, || f.remove(stage));
        assert!(hit);
        assert_eq!(result.receipt().effect, RemovalEffect::Deleted);
        assert_eq!(result.receipt().cleanup, Cleanup::Pending);
        assert!(path.is_dir());
        fs::write(&f.raw, b"new").unwrap();
        assert_eq!(result.cleanup(), Cleanup::Complete);
        assert_eq!(
            result.receipt().stage_parent_durability,
            Durability::Unproven
        );
        assert_eq!(fs::read(&f.raw).unwrap(), b"new");
        assert!(!path.exists());
    }

    #[test]
    fn stage_recreation_after_cleanup_invalidates_confirmation_and_preserves_competitor() {
        let f = Fixture::new();
        let stage = f.stage();
        let path = stage.diagnostic_path();
        let recreated = path.clone();
        let raw = f.raw.clone();
        let (mut result, hit) = run_with_media_hook(
            MediaPrimitive::RemovalParentSync,
            move || {
                fs::create_dir(&recreated)?;
                fs::write(recreated.join("raw.flac"), b"foreign")?;
                fs::write(raw, b"new")
            },
            || f.remove(stage),
        );
        assert!(hit);
        assert_eq!(result.receipt().effect, RemovalEffect::Deleted);
        assert_eq!(
            result.receipt().claim_confirmation,
            Confirmation::Unverified
        );
        assert_eq!(result.receipt().original_name, OriginalName::Occupied);
        assert_eq!(result.receipt().cleanup, Cleanup::Refused);
        assert_eq!(result.cleanup(), Cleanup::Refused);
        drop(result);
        assert_eq!(fs::read(path.join("raw.flac")).unwrap(), b"foreign");
        assert_eq!(fs::read(&f.raw).unwrap(), b"new");
    }

    #[cfg(windows)]
    #[test]
    fn retained_removal_handle_excludes_write_delete_rename_until_release() {
        let f = Fixture::new();
        let raw = f.raw.clone();
        let renamed = raw.with_extension("renamed");
        let (result, hit) = run_with_media_hook(
            MediaPrimitive::BeforeRemovalClaim,
            move || {
                assert!(fs::write(&raw, b"xyz").is_err());
                assert!(fs::remove_file(&raw).is_err());
                assert!(fs::rename(&raw, renamed).is_err());
                fault()
            },
            || f.remove(f.stage()),
        );
        assert!(hit);
        assert_eq!(result.receipt().effect, RemovalEffect::NotClaimed);
        drop(result);
        fs::write(&f.raw, b"xyz").unwrap();
        fs::rename(&f.raw, f.raw.with_extension("renamed")).unwrap();
        fs::remove_file(f.raw.with_extension("renamed")).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn foreign_reader_keeps_accepted_deletion_pending_without_cleanup_authority() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows_sys::Win32::Storage::FileSystem::{
            FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        let f = Fixture::new();
        let reader = fs::OpenOptions::new()
            .read(true)
            .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE)
            .open(&f.raw)
            .unwrap();
        let stage = f.stage();
        let path = stage.diagnostic_path();
        let mut result = f.remove(stage);
        assert_eq!(
            result.receipt().effect,
            RemovalEffect::DeletePending,
            "{:?}",
            result.receipt()
        );
        assert_eq!(result.cleanup(), Cleanup::Refused);
        assert!(path.is_dir());
        drop(reader);
        assert_eq!(result.cleanup(), Cleanup::Refused);
        drop(result);
        assert!(path.is_dir());
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod removal_rebinding {
    use super::*;
    use std::fs;

    #[test]
    fn changed_source_or_stage_parent_after_mutation_preserves_actual_effects() {
        for (cut, replace_parent) in [
            (MediaPrimitive::AfterRemovalClaim, false),
            (MediaPrimitive::AfterRemovalClaim, true),
            (MediaPrimitive::RemovalCompletion, false),
            (MediaPrimitive::RemovalCompletion, true),
        ] {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir_all(temp.path().join("parent/segment")).unwrap();
            fs::write(temp.path().join("parent/segment/raw.flac"), b"abc").unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
            let parent = MediaDirectory::open(root, Path::new("parent")).unwrap();
            let source = parent.open_child(OsStr::new("segment")).unwrap().unwrap();
            let expected = source
                .observe_file(OsStr::new("raw.flac"))
                .unwrap()
                .unwrap();
            let stage = MediaStage::create(Arc::clone(&parent), activity.clone()).unwrap();
            let stage_name = stage.name.clone();
            let target = if replace_parent {
                temp.path().join("parent")
            } else {
                temp.path().join("parent/segment")
            };
            let saved = target.with_extension("saved");
            let target_hook = target.clone();
            let saved_hook = saved.clone();
            let (mut result, hit) = run_with_media_hook(
                cut,
                move || {
                    fs::rename(&target_hook, saved_hook)?;
                    fs::create_dir(&target_hook)?;
                    fs::write(target_hook.join("sentinel"), b"competitor")
                },
                || source.remove_observed(OsStr::new("raw.flac"), &expected, stage),
            );
            assert!(hit);
            assert_eq!(
                result.receipt().effect,
                if cut == MediaPrimitive::AfterRemovalClaim {
                    RemovalEffect::Claimed
                } else {
                    RemovalEffect::DeletePending
                }
            );
            assert_eq!(
                result.receipt().claim_confirmation,
                Confirmation::Unverified
            );
            assert_eq!(result.cleanup(), Cleanup::Refused);
            assert_eq!(fs::read(target.join("sentinel")).unwrap(), b"competitor");
            let retained_stage = if replace_parent {
                saved.join(&stage_name)
            } else {
                temp.path().join("parent").join(&stage_name)
            };
            drop(result);
            assert!(retained_stage.is_dir());
            if cut == MediaPrimitive::AfterRemovalClaim {
                assert_eq!(
                    fs::read(retained_stage.join(super::REMOVAL_ORIGINAL_LEAF)).unwrap(),
                    b"abc"
                );
            }
        }
    }

    #[test]
    fn replaced_stage_and_preclaim_source_are_preserved() {
        for cut in [
            MediaPrimitive::BeforeRemovalClaim,
            MediaPrimitive::AfterRemovalClaim,
        ] {
            let temp = tempfile::tempdir().unwrap();
            fs::create_dir_all(temp.path().join("parent/segment")).unwrap();
            fs::write(temp.path().join("parent/segment/raw.flac"), b"abc").unwrap();
            let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
            let activity = crate::activity::ActivityLease::try_acquire(root.clone()).unwrap();
            let parent = MediaDirectory::open(root, Path::new("parent")).unwrap();
            let source = parent.open_child(OsStr::new("segment")).unwrap().unwrap();
            let expected = source
                .observe_file(OsStr::new("raw.flac"))
                .unwrap()
                .unwrap();
            let stage = MediaStage::create(parent, activity.clone()).unwrap();
            let target = if cut == MediaPrimitive::BeforeRemovalClaim {
                source.diagnostic_path().to_owned()
            } else {
                stage.diagnostic_path()
            };
            let old = target.with_extension("saved");
            let target_hook = target.clone();
            let old_hook = old.clone();
            let (mut result, hit) = run_with_media_hook(
                cut,
                move || {
                    fs::rename(&target_hook, old_hook)?;
                    fs::create_dir(&target_hook)?;
                    fs::write(target_hook.join("raw.flac"), b"foreign")
                },
                || source.remove_observed(OsStr::new("raw.flac"), &expected, stage),
            );
            assert!(hit);
            assert_eq!(
                result.receipt().effect,
                if cut == MediaPrimitive::BeforeRemovalClaim {
                    RemovalEffect::NotClaimed
                } else {
                    RemovalEffect::Claimed
                }
            );
            if cut == MediaPrimitive::AfterRemovalClaim {
                assert_eq!(result.cleanup(), Cleanup::Refused);
            }
            drop(result);
            assert_eq!(fs::read(target.join("raw.flac")).unwrap(), b"foreign");
            let moved = if cut == MediaPrimitive::BeforeRemovalClaim {
                "raw.flac"
            } else {
                super::REMOVAL_ORIGINAL_LEAF
            };
            assert_eq!(fs::read(old.join(moved)).unwrap(), b"abc");
        }
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(windows)]
mod removal_other_volume {
    use super::*;
    use std::fs;

    #[test]
    #[ignore = "requires SOLSTONE_MEDIA_OTHER_ROOT on a separate Windows volume"]
    fn cross_volume_stage_refuses_without_copy_or_source_effect() {
        let temp = tempfile::tempdir().unwrap();
        fs::create_dir_all(temp.path().join("parent/segment")).unwrap();
        fs::write(temp.path().join("parent/segment/raw.flac"), b"abc").unwrap();
        let root = Arc::new(JournalRoot::open(temp.path()).unwrap());
        let source = MediaDirectory::open(root, Path::new("parent/segment")).unwrap();
        let expected = source
            .observe_file(OsStr::new("raw.flac"))
            .unwrap()
            .unwrap();
        let other =
            tempfile::tempdir_in(std::env::var_os("SOLSTONE_MEDIA_OTHER_ROOT").unwrap()).unwrap();
        let other_root = Arc::new(JournalRoot::open(other.path()).unwrap());
        let other_activity =
            crate::activity::ActivityLease::try_acquire(other_root.clone()).unwrap();
        let parent = MediaDirectory::open(other_root, Path::new("")).unwrap();
        assert!(!source.identity().same_volume(parent.identity()));
        let mut result = source.remove_observed(
            OsStr::new("raw.flac"),
            &expected,
            MediaStage::create(parent, other_activity).unwrap(),
        );
        assert_eq!(result.receipt().effect, RemovalEffect::NotClaimed);
        assert_eq!(result.cleanup(), Cleanup::Complete);
        assert_eq!(
            fs::read(temp.path().join("parent/segment/raw.flac")).unwrap(),
            b"abc"
        );
        assert_eq!(
            fs::read_dir(other.path())
                .unwrap()
                .map(|e| e.unwrap().file_name())
                .collect::<Vec<_>>(),
            vec![OsString::from("backup-activity.lock")]
        );
    }
}
