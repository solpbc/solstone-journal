// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(feature = "full-tests")]
use std::collections::HashMap;
#[cfg(feature = "full-tests")]
use std::fs;
#[cfg(feature = "full-tests")]
use std::time::Duration;

use chrono_tz::Tz;
#[cfg(feature = "full-tests")]
use sha2::{Digest, Sha256};
#[cfg(feature = "full-tests")]
use solstone_core_format::audio_archive::AUDIO_ARCHIVE_PLACEMENT_FILENAME;
#[cfg(feature = "full-tests")]
use tempfile::TempDir;

use super::*;

const NY: Tz = Tz::America__New_York;
const UTC_ZONE: Tz = Tz::UTC;

#[cfg(feature = "full-tests")]
#[test]
fn archive_run_lookup_is_read_only_when_absent_and_survives_publication_interruption() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let identity = default_settings().archive_identity;
    assert_eq!(read_matching_archive_run(journal, &identity).unwrap(), None);
    assert!(!journal.join("imports").exists());
    let (member, bytes) = storage_member(231, "2026-01-14T21:00:00Z", 7500);
    let materials = HashMap::from([(member.sha256.clone(), bytes)]);
    assert!(matches!(
        storage_publish(
            journal,
            None,
            &[member],
            &materials,
            Some(ArchiveFault::AfterRecordFirstSaved)
        ),
        Err(AudioArchiveError::Fault(
            ArchiveFault::AfterRecordFirstSaved
        ))
    ));
    let id = storage_run(journal);
    // Unrelated incomplete runs are deliberately not opened by this keyed lookup.
    fs::create_dir(journal.join("imports/20200101_010101")).unwrap();
    let lookup_path = claim_db_path(journal);
    let before = fs::read(&lookup_path).unwrap();
    let modified = fs::metadata(&lookup_path).unwrap().modified().unwrap();
    assert_eq!(
        read_matching_archive_run(journal, &identity).unwrap(),
        Some(id.clone())
    );
    assert_eq!(
        read_matching_archive_run(journal, "another-archive").unwrap(),
        None
    );
    assert_eq!(fs::read(&lookup_path).unwrap(), before);
    assert_eq!(
        fs::metadata(&lookup_path).unwrap().modified().unwrap(),
        modified
    );
    fs::remove_dir_all(journal.join("imports").join(&id)).unwrap();
    assert_eq!(read_matching_archive_run(journal, &identity).unwrap(), None);
    assert!(claim_db_path(journal).is_file());
}

#[cfg(feature = "full-tests")]
#[test]
fn archive_run_lookup_does_not_hide_a_damaged_living_run_or_inherit_a_replacement() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let identity = default_settings().archive_identity;
    let (member, bytes) = storage_member(232, "2026-01-14T21:00:00Z", 7500);
    let materials = HashMap::from([(member.sha256.clone(), bytes)]);
    let result = storage_publish(journal, None, &[member], &materials, None).unwrap();
    let path = record::audio_archive_record_path(journal, &result.import_id);
    fs::remove_file(&path).unwrap();
    assert!(read_matching_archive_run(journal, &identity).is_err());
    let mut replacement = result.record;
    replacement.settings.archive_identity = "different-archive".to_owned();
    save_audio_archive_record(journal, &result.import_id, &replacement).unwrap();
    assert_eq!(read_matching_archive_run(journal, &identity).unwrap(), None);
    claims::record_archive_run(journal, &result.import_id, "different-archive").unwrap();
    assert_eq!(
        read_matching_archive_run(journal, "different-archive").unwrap(),
        Some(result.import_id)
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn retry_rechecks_another_runs_ownership_of_previously_rejected_member() {
    struct Unreadable;
    impl AudioArchiveMaterializer for Unreadable {
        fn materialize(
            &self,
            _member: &AudioArchiveRecordMember,
            _part: &AudioArchiveRecordPart,
            _target: &std::path::Path,
        ) -> Result<(), AudioArchiveError> {
            Err(AudioArchiveError::SourceRejected(
                SourceRejection::Unreadable,
            ))
        }
    }
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (member, bytes) = storage_member(243, "2026-01-14T21:00:00Z", 7500);
    let materials = HashMap::from([(member.sha256.clone(), bytes)]);
    let rejected = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &Unreadable,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();
    let owner = storage_publish(
        journal,
        None,
        std::slice::from_ref(&member),
        &materials,
        None,
    )
    .unwrap();
    let raw_part = &owner.record.parts[0];
    let raw = journal
        .join("chronicle")
        .join(&raw_part.day)
        .join(&owner.record.settings.stream)
        .join(&raw_part.segment_key)
        .join(&raw_part.stored_name);
    let before = fs::metadata(&raw).unwrap().modified().unwrap();
    let retried = storage_publish(
        journal,
        Some(&rejected.import_id),
        &[member],
        &materials,
        None,
    )
    .unwrap();
    assert_eq!(retried.parts_saved, 0);
    assert_eq!(
        retried.record.members[0].save_rejection,
        Some(SourceRejection::HeldByExistingRun)
    );
    assert_eq!(retried.record.parts[0].outcome, PartOutcome::Rejected);
    assert_eq!(fs::metadata(raw).unwrap().modified().unwrap(), before);
    let status = ArchiveStore::open(journal, &rejected.import_id, false)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(
        (
            status.saved_parts,
            status.rejected_parts,
            status.pending_processing
        ),
        (0, 1, 0)
    );
    assert!(status.save_complete);
}

#[cfg(feature = "full-tests")]
#[test]
fn restored_source_resume_fills_only_rejected_unpublished_parts() {
    struct RejectLater {
        materials: HashMap<String, Vec<u8>>,
        bad: String,
    }
    impl AudioArchiveMaterializer for RejectLater {
        fn materialize(
            &self,
            member: &AudioArchiveRecordMember,
            part: &AudioArchiveRecordPart,
            target: &std::path::Path,
        ) -> Result<(), AudioArchiveError> {
            if member.sha256 == self.bad && part.origin_ms > 0 {
                return Err(AudioArchiveError::SourceRejected(
                    SourceRejection::Unreadable,
                ));
            }
            self.materials.materialize(member, part, target)
        }
    }
    for fault in [
        None,
        Some(ArchiveFault::BeforeTilePublish),
        Some(ArchiveFault::AfterPublishBeforeSavedMark),
    ] {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();
        let (member, bytes) = storage_member(241, "2026-01-14T21:00:00Z", 400_000);
        let (neighbor, neighbor_bytes) = storage_member(242, "2026-01-14T21:06:50Z", 7500);
        let sources = RejectLater {
            materials: HashMap::from([
                (member.sha256.clone(), bytes.clone()),
                (neighbor.sha256.clone(), neighbor_bytes),
            ]),
            bad: member.sha256.clone(),
        };
        let members = if fault.is_some() {
            vec![member.clone(), neighbor]
        } else {
            vec![member.clone()]
        };
        assert!(matches!(
            storage_publish(
                journal,
                None,
                &members,
                &sources.materials,
                Some(ArchiveFault::BetweenMultiTileParts)
            ),
            Err(AudioArchiveError::Fault(_))
        ));
        let id = storage_run(journal);
        let record = read_audio_archive(journal, &id).unwrap().unwrap();
        let first = &record.parts[0];
        let raw = journal
            .join("chronicle")
            .join(&first.day)
            .join(&record.settings.stream)
            .join(&first.segment_key)
            .join(&first.stored_name);
        let before = fs::metadata(&raw).unwrap();
        let rejected = publish_audio_archive(
            journal,
            AudioArchivePublishOptions {
                import_id: Some(&id),
                members: vec![],
                materializer: &sources,
                owner_zone: NY,
                settings: default_settings(),
                source_timestamp: None,
            },
            Some(AudioArchivePublishSeams {
                fault,
                emit_observed: None,
                heartbeat_spawner: None,
                progress: None,
            }),
        );
        if fault.is_some() {
            assert!(matches!(rejected, Err(AudioArchiveError::Fault(_))));
        } else {
            assert!(rejected.is_ok());
        }
        let restored =
            storage_publish(journal, Some(&id), &members, &sources.materials, None).unwrap();
        let status = ArchiveStore::open(journal, &id, false)
            .unwrap()
            .status()
            .unwrap();
        let published_gap = fault == Some(ArchiveFault::AfterPublishBeforeSavedMark);
        let expected_saved = if fault == Some(ArchiveFault::BeforeTilePublish) {
            3
        } else {
            2
        };
        assert_eq!(
            (
                status.saved_parts,
                status.rejected_parts,
                status.pending_processing
            ),
            (expected_saved, u64::from(published_gap), expected_saved)
        );
        assert!(status.save_complete);
        assert_eq!(
            restored
                .record
                .parts
                .iter()
                .filter(|part| part.outcome == PartOutcome::Rejected)
                .count(),
            usize::from(published_gap)
        );
        assert_eq!(fs::read(&raw).unwrap(), bytes);
        assert_eq!(
            fs::metadata(&raw).unwrap().modified().unwrap(),
            before.modified().unwrap()
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(fs::metadata(&raw).unwrap().ino(), before.ino());
        }
    }
}

#[cfg(feature = "full-tests")]
#[test]
fn same_tile_multi_part_rejection_prunes_staged_bytes_and_skips_stale_callbacks() {
    struct Rejecting {
        materials: HashMap<String, Vec<u8>>,
        bad: String,
        reject_zero: bool,
        calls: std::cell::Cell<usize>,
    }
    impl AudioArchiveMaterializer for Rejecting {
        fn materialize(
            &self,
            member: &AudioArchiveRecordMember,
            part: &AudioArchiveRecordPart,
            target: &std::path::Path,
        ) -> Result<(), AudioArchiveError> {
            if member.sha256 == self.bad {
                self.calls.set(self.calls.get() + 1);
                if (part.origin_ms == 0) == self.reject_zero {
                    fs::write(target, b"partial failed source bytes")?;
                    return Err(AudioArchiveError::SourceRejected(SourceRejection::Changed));
                }
            }
            self.materials.materialize(member, part, target)
        }
    }
    for reject_zero in [false, true] {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();
        let (a, a_bytes) = storage_member(231, "2026-11-01T01:00:20-04:00", 3_590_000);
        let (b, b_bytes) = storage_member(232, "2026-11-01T01:00:11-04:00", 7500);
        let materializer = Rejecting {
            materials: HashMap::from([
                (a.sha256.clone(), a_bytes),
                (b.sha256.clone(), b_bytes.clone()),
            ]),
            bad: a.sha256.clone(),
            reject_zero,
            calls: std::cell::Cell::new(0),
        };
        let result = publish_audio_archive(
            journal,
            AudioArchivePublishOptions {
                import_id: None,
                members: vec![a, b],
                materializer: &materializer,
                owner_zone: NY,
                settings: default_settings(),
                source_timestamp: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(materializer.calls.get(), if reject_zero { 2 } else { 1 });
        let status = ArchiveStore::open(journal, &result.import_id, false)
            .unwrap()
            .status()
            .unwrap();
        assert!(status.save_complete);
        assert_eq!(
            (
                status.saved_parts,
                status.rejected_parts,
                status.saved_tiles
            ),
            (1, 13, 1)
        );
        let healthy = result
            .record
            .parts
            .iter()
            .find(|part| part.outcome == PartOutcome::Saved)
            .unwrap();
        let tile = journal
            .join("chronicle")
            .join(&healthy.day)
            .join(&result.record.settings.stream)
            .join(&healthy.segment_key);
        assert_eq!(fs::read(tile.join(&healthy.stored_name)).unwrap(), b_bytes);
        let placement: solstone_core_format::audio_archive::AudioArchivePlacement =
            serde_json::from_slice(&fs::read(tile.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME)).unwrap())
                .unwrap();
        assert_eq!(placement.parts.len(), 1);
        assert_eq!(
            fs::read_dir(&tile)
                .unwrap()
                .filter(|entry| entry
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|ext| ext == "ogg"))
                .count(),
            1
        );
        let batch = update_pending_archive_tiles(journal, 64).unwrap();
        assert!(batch.errors.is_empty());
        assert_eq!(batch.tiles.len(), 1);
        assert_eq!(batch.tiles[0].parts.len(), 1);
    }
}

#[test]
fn rejected_probe_preserves_source_evidence_without_reserving_a_tile() {
    let input = AudioArchiveMemberInput {
        original_relative_name: "unreadable.ogg".into(),
        byte_len: 123,
        sha256: String::new(),
        duration: DurationInput::Rejected(SourceRejection::Unreadable),
        start: MemberStartEvidence {
            instant: Some("2025-06-01T08:00:00-06:00".into()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(std::slice::from_ref(&input), NY, default_settings()).unwrap();
    assert!(plan.tiles.is_empty());
    assert_eq!(plan.size.source_bytes, 123);
    assert_eq!(plan.members[0].source_start, input.start);
    assert_eq!(
        plan.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::SourceRejected {
            cause: SourceRejection::Unreadable
        })
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn source_rejections_preserve_saved_long_parts_and_do_not_block_other_members() {
    struct Rejecting {
        materials: HashMap<String, Vec<u8>>,
        long: String,
        unreadable: String,
    }
    impl AudioArchiveMaterializer for Rejecting {
        fn materialize(
            &self,
            member: &AudioArchiveRecordMember,
            part: &AudioArchiveRecordPart,
            target: &std::path::Path,
        ) -> Result<(), AudioArchiveError> {
            if member.sha256 == self.long && part.origin_ms > 0 {
                return Err(AudioArchiveError::SourceRejected(SourceRejection::Changed));
            }
            if member.sha256 == self.unreadable {
                return Err(AudioArchiveError::SourceRejected(
                    SourceRejection::Unreadable,
                ));
            }
            self.materials.materialize(member, part, target)
        }
    }
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let sources = [
        storage_member(201, "2026-01-14T21:00:00Z", 400_000),
        storage_member(202, "2026-01-14T21:06:50Z", 7500),
        storage_member(203, "2026-01-14T21:10:00Z", 7500),
        storage_member(204, "2026-01-14T21:10:15Z", 7500),
    ];
    let members: Vec<_> = sources.iter().map(|(member, _)| member.clone()).collect();
    let materializer = Rejecting {
        materials: sources
            .iter()
            .map(|(member, bytes)| (member.sha256.clone(), bytes.clone()))
            .collect(),
        long: members[0].sha256.clone(),
        unreadable: members[2].sha256.clone(),
    };
    let result = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: members.clone(),
            materializer: &materializer,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();
    let status = ArchiveStore::open(journal, &result.import_id, false)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(
        (
            status.saved_parts,
            status.rejected_parts,
            status.saved_members,
            status.pending_processing
        ),
        (3, 2, 2, 3)
    );
    assert!(status.save_complete);
    assert_eq!(
        result.record.members[0].save_rejection,
        Some(SourceRejection::Changed)
    );
    assert_eq!(
        result.record.members[2].save_rejection,
        Some(SourceRejection::Unreadable)
    );
    assert_eq!(
        result
            .record
            .parts
            .iter()
            .filter(|part| part.outcome == PartOutcome::Saved)
            .count(),
        3
    );
    let batch = update_pending_archive_tiles(journal, 64).unwrap();
    assert!(batch.errors.is_empty());
    assert_eq!(
        batch
            .tiles
            .iter()
            .map(|tile| tile.parts.len())
            .sum::<usize>(),
        3
    );
    let snapshots: Vec<_> = result
        .record
        .parts
        .iter()
        .filter(|part| part.outcome == PartOutcome::Saved)
        .map(|part| {
            let raw = journal
                .join("chronicle")
                .join(&part.day)
                .join(&result.record.settings.stream)
                .join(&part.segment_key)
                .join(&part.stored_name);
            (
                raw.clone(),
                fs::metadata(&raw).unwrap().modified().unwrap(),
                fs::read(&raw).unwrap(),
            )
        })
        .collect();
    let resumed = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&result.import_id),
            members,
            materializer: &materializer,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();
    assert_eq!(resumed.record, result.record);
    for (raw, modified, bytes) in snapshots {
        assert_eq!(fs::read(&raw).unwrap(), bytes);
        assert_eq!(fs::metadata(raw).unwrap().modified().unwrap(), modified);
    }
}

#[cfg(feature = "full-tests")]
#[test]
fn reused_timestamp_does_not_inherit_removed_run_claims_or_pending_coordinates() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (a, a_bytes) = storage_member(211, "2026-01-14T21:00:00Z", 7500);
    let (b, b_bytes) = storage_member(212, "2026-01-14T22:00:00Z", 7500);
    let materializer = HashMap::from([(a.sha256.clone(), a_bytes), (b.sha256.clone(), b_bytes)]);
    let run = |members: Vec<_>, qualifier: &str| {
        publish_audio_archive(
            journal,
            AudioArchivePublishOptions {
                import_id: None,
                members,
                materializer: &materializer,
                owner_zone: NY,
                settings: AudioArchiveSettings {
                    qualifier: qualifier.into(),
                    archive_identity: qualifier.into(),
                },
                source_timestamp: Some(
                    crate::timestamp::validate_timestamp("20260115_000000").unwrap(),
                ),
            },
            None,
        )
        .unwrap()
    };
    let first = run(vec![a.clone()], "original");
    fs::remove_dir_all(journal.join("imports").join(&first.import_id)).unwrap();
    let replacement = run(vec![b], "replacement");
    assert_eq!(first.import_id, replacement.import_id);
    let mut settings = default_settings();
    settings.qualifier = "released".into();
    let preview =
        prepare_audio_archive_plan(journal, std::slice::from_ref(&a), NY, settings).unwrap();
    assert_eq!(preview.members[0].disposition, MemberDisposition::Included);
    let released = run(vec![a], "released");
    assert_eq!(released.parts_saved, 1);
    let batch = update_pending_archive_tiles(journal, 64).unwrap();
    assert!(batch.errors.is_empty());
    assert_eq!(batch.tiles.len(), 2);
    assert!(
        batch
            .tiles
            .iter()
            .all(|tile| tile.stream != "import.original")
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn damaged_newer_pending_prefix_cannot_starve_healthy_older_work() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (member, bytes) = storage_member(221, "2025-01-14T21:00:00Z", 7500);
    let materials = HashMap::from([(member.sha256.clone(), bytes)]);
    let healthy = storage_publish(journal, None, &[member], &materials, None).unwrap();
    let bad_id = "20260120_000000";
    fs::create_dir_all(journal.join("imports").join(bad_id)).unwrap();
    fs::write(
        journal
            .join("imports")
            .join(bad_id)
            .join(AUDIO_ARCHIVE_RECORD_FILENAME),
        b"not a sqlite database",
    )
    .unwrap();
    for ordinal in 0..64 {
        let minutes = ordinal * 5;
        let key = format!("{:02}{:02}00_300", minutes / 60, minutes % 60);
        super::claims::register_pending_tile(journal, bad_id, "20260120", &key).unwrap();
    }
    let first = update_pending_archive_tiles(journal, 64).unwrap();
    assert_eq!(first.errors.len(), 64);
    assert!(first.tiles.is_empty());
    let second = update_pending_archive_tiles(journal, 64).unwrap();
    assert!(
        second
            .tiles
            .iter()
            .any(|tile| tile.import_id == healthy.import_id)
    );
}

fn default_settings() -> AudioArchiveSettings {
    AudioArchiveSettings {
        qualifier: "clips".to_owned(),
        archive_identity: "archive_test_run_1".to_owned(),
    }
}

fn dummy_hash(idx: u8) -> String {
    format!("{:02x}", idx).repeat(32)
}

#[test]
fn equal_bytes_at_two_times_in_one_tile_have_independent_raw_targets() {
    let first = AudioArchiveMemberInput {
        original_relative_name: "one.ogg".to_owned(),
        byte_len: 100,
        sha256: dummy_hash(91),
        duration: DurationInput::Milliseconds(7500),
        start: MemberStartEvidence {
            instant: Some("2025-06-01T08:00:00-06:00".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let mut second = first.clone();
    second.original_relative_name = "renamed.ogg".to_owned();
    second.start.instant = Some("2025-06-01T08:00:12.500-06:00".to_owned());
    let plan =
        plan_audio_archive(&[first, second], Tz::America__Denver, default_settings()).unwrap();
    assert_eq!(plan.tiles.len(), 1);
    assert_eq!(plan.tiles[0].parts.len(), 2);
    assert_ne!(
        plan.tiles[0].parts[0].stored_name,
        plan.tiles[0].parts[1].stored_name
    );
    assert_eq!(plan.tiles[0].parts[1].offset_ms, 12_500);
}

#[test]
fn held_member_reserves_no_interval_for_a_different_archive_stream() {
    let first = AudioArchiveMemberInput {
        original_relative_name: "held.ogg".to_owned(),
        byte_len: 100,
        sha256: dummy_hash(92),
        duration: DurationInput::Milliseconds(7500),
        start: MemberStartEvidence {
            instant: Some("2025-06-01T08:00:00-06:00".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let mut second = first.clone();
    second.sha256 = dummy_hash(93);
    second.original_relative_name = "other.ogg".to_owned();
    let mut plan =
        plan_audio_archive(&[first, second], Tz::America__Denver, default_settings()).unwrap();
    let held = std::collections::HashSet::from([plan.members[0].identity.clone()]);
    super::plan::apply_held_members(&mut plan, &held).unwrap();
    assert_eq!(
        plan.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::HeldByExistingRun)
    );
    assert_eq!(plan.members[1].disposition, MemberDisposition::Included);
    assert_eq!(
        plan.tiles[0].parts[0].member_identity.sha256,
        dummy_hash(93)
    );
}

#[test]
fn run_record_preserves_unsettled_evidence_and_source_population() {
    let input = AudioArchiveMemberInput {
        original_relative_name: "unknown.ogg".to_owned(),
        byte_len: 123,
        sha256: dummy_hash(94),
        duration: DurationInput::Milliseconds(7500),
        start: MemberStartEvidence {
            local_civil: Some("2025-11-02T01:58:00".to_owned()),
            authority: MemberTimeAuthority::Filename,
            ..Default::default()
        },
        run_note: Some("owner note".to_owned()),
    };
    let plan = plan_audio_archive(
        std::slice::from_ref(&input),
        Tz::America__Denver,
        default_settings(),
    )
    .unwrap();
    let record = super::record::plan_to_record(&plan);
    assert_eq!(record.members[0].source_start, input.start);
    assert_eq!(record.members[0].duration_ms, 7500);
    assert_eq!(record.members[0].run_note, input.run_note);
    assert_eq!(plan.size.source_bytes, 123);
    assert_eq!(plan.size.members, 1);
    assert_eq!(plan.size.bytes, 0);
}

#[cfg(feature = "full-tests")]
fn storage_member(
    seed: u32,
    instant: &str,
    duration_ms: i64,
) -> (AudioArchiveMemberInput, Vec<u8>) {
    let bytes = format!("synthetic storage payload {seed}").into_bytes();
    let member = AudioArchiveMemberInput {
        original_relative_name: format!("member-{seed}.ogg"),
        byte_len: bytes.len() as u64,
        sha256: format!("{:x}", Sha256::digest(&bytes)),
        duration: DurationInput::Milliseconds(duration_ms),
        start: MemberStartEvidence {
            instant: Some(instant.to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    (member, bytes)
}
#[cfg(feature = "full-tests")]
fn storage_publish(
    journal: &std::path::Path,
    id: Option<&str>,
    members: &[AudioArchiveMemberInput],
    bytes: &HashMap<String, Vec<u8>>,
    fault: Option<ArchiveFault>,
) -> Result<AudioArchivePublishResult, AudioArchiveError> {
    publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: id,
            members: members.to_vec(),
            materializer: bytes,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(AudioArchivePublishSeams {
            fault,
            emit_observed: None,
            heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
            progress: None,
        }),
    )
}
#[cfg(feature = "full-tests")]
fn storage_run(journal: &std::path::Path) -> String {
    let ids: Vec<_> = fs::read_dir(journal.join("imports"))
        .unwrap()
        .map(|entry| entry.unwrap())
        .filter(|entry| entry.file_type().unwrap().is_dir())
        .map(|entry| entry.file_name().into_string().unwrap())
        .collect();
    assert_eq!(ids.len(), 1);
    ids[0].clone()
}

#[test]
fn pure_plan_bad_durations_and_unsettled_times() {
    let inputs = vec![
        AudioArchiveMemberInput {
            original_relative_name: "archive/empty.ogg".to_owned(),
            byte_len: 100,
            sha256: dummy_hash(1),
            duration: DurationInput::Empty,
            start: MemberStartEvidence {
                instant: Some("2026-01-15T10:00:00.500Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        },
        AudioArchiveMemberInput {
            original_relative_name: "archive/nonfinite.ogg".to_owned(),
            byte_len: 100,
            sha256: dummy_hash(2),
            duration: DurationInput::NonFinite,
            start: MemberStartEvidence {
                instant: Some("2026-01-15T10:00:00.500Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        },
        AudioArchiveMemberInput {
            original_relative_name: "archive/zero.ogg".to_owned(),
            byte_len: 100,
            sha256: dummy_hash(3),
            duration: DurationInput::Milliseconds(0),
            start: MemberStartEvidence {
                instant: Some("2026-01-15T10:00:00.500Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        },
        AudioArchiveMemberInput {
            original_relative_name: "archive/negative.ogg".to_owned(),
            byte_len: 100,
            sha256: dummy_hash(4),
            duration: DurationInput::Milliseconds(-500),
            start: MemberStartEvidence {
                instant: Some("2026-01-15T10:00:00.500Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        },
        AudioArchiveMemberInput {
            original_relative_name: "archive/no_evidence.ogg".to_owned(),
            byte_len: 100,
            sha256: dummy_hash(5),
            duration: DurationInput::Milliseconds(1000),
            start: MemberStartEvidence {
                instant: None,
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        },
        AudioArchiveMemberInput {
            original_relative_name: "archive/no_offset.ogg".to_owned(),
            byte_len: 100,
            sha256: dummy_hash(6),
            duration: DurationInput::Milliseconds(1000),
            start: MemberStartEvidence {
                instant: Some("2026-01-15T10:00:00.500".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        },
    ];

    let plan = plan_audio_archive(&inputs, NY, default_settings()).unwrap();
    assert_eq!(plan.members.len(), 6);
    assert_eq!(plan.tiles.len(), 0);
    assert_eq!(plan.size.bytes, 0);
    assert_eq!(plan.size.members, 6);
    assert_eq!(plan.size.tiles, 0);

    assert_eq!(
        plan.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::BadDuration)
    );
    assert_eq!(
        plan.members[1].disposition,
        MemberDisposition::LeftOut(LeftOutReason::BadDuration)
    );
    assert_eq!(
        plan.members[2].disposition,
        MemberDisposition::LeftOut(LeftOutReason::BadDuration)
    );
    assert_eq!(
        plan.members[3].disposition,
        MemberDisposition::LeftOut(LeftOutReason::BadDuration)
    );
    assert_eq!(
        plan.members[4].disposition,
        MemberDisposition::LeftOut(LeftOutReason::UnsettledTime)
    );
    assert_eq!(
        plan.members[5].disposition,
        MemberDisposition::LeftOut(LeftOutReason::UnsettledTime)
    );
}

#[test]
fn subsecond_and_short_durations() {
    // Under 1s duration (500 ms) and 7_500 ms
    let inputs = vec![AudioArchiveMemberInput {
        original_relative_name: "archive/half_second.ogg".to_owned(),
        byte_len: 200,
        sha256: dummy_hash(10),
        duration: DurationInput::Milliseconds(500),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.500Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    }];

    let plan = plan_audio_archive(&inputs, NY, default_settings()).unwrap();
    assert_eq!(plan.tiles.len(), 1);
    assert_eq!(plan.tiles[0].parts.len(), 1);
    let p = &plan.tiles[0].parts[0];
    assert_eq!(p.length_ms, 500);
    assert_eq!(p.offset_ms, 500);
    assert_eq!(p.origin_ms, 0);
}

#[test]
fn worked_fixtures_hardcoded_numbers() {
    // Short Auckland member:
    // Auckland 2026-01-15T10:00:00.500, duration 7_500 ms.
    // NZDT = UTC+13 -> UTC = 2026-01-14T21:00:00.500Z.
    // Epoch arithmetic: 2026-01-14 21:00:00 UTC is 1768424400s -> 1_768_424_400_500 ms.
    // America/New_York is UTC-5 -> 2026-01-14 16:00:00.500.
    // Day 20260114, key 160000_300, offset 500, origin 0, length 7500.
    let short_member = AudioArchiveMemberInput {
        original_relative_name: "archive/short.ogg".to_owned(),
        byte_len: 1024,
        sha256: dummy_hash(20),
        duration: DurationInput::Milliseconds(7_500),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-01-15T10:00:00.500".to_owned()),
            source_zone: Some("Pacific/Auckland".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };

    // Long Auckland member:
    // Auckland 2026-01-15T18:07:30.250, duration 400_000 ms.
    // UTC = 2026-01-15T05:07:30.250Z.
    // America/New_York (UTC-5) = 2026-01-15 00:07:30.250.
    // Part 0: Day 20260115, key 000500_300, offset 150250, origin 0, length 149750.
    // Part 1: Day 20260115, key 001000_300, offset 0, origin 149750, length 250250.
    let long_member = AudioArchiveMemberInput {
        original_relative_name: "archive/long.ogg".to_owned(),
        byte_len: 2048,
        sha256: dummy_hash(21),
        duration: DurationInput::Milliseconds(400_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-01-15T18:07:30.250".to_owned()),
            source_zone: Some("Pacific/Auckland".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };

    let plan = plan_audio_archive(&[short_member, long_member], NY, default_settings()).unwrap();
    assert_eq!(plan.tiles.len(), 3);

    // Tile 0: Short member
    assert_eq!(plan.tiles[0].day, "20260114");
    assert_eq!(plan.tiles[0].segment_key, "160000_300");
    assert_eq!(plan.tiles[0].parts.len(), 1);
    assert_eq!(plan.tiles[0].parts[0].offset_ms, 500);
    assert_eq!(plan.tiles[0].parts[0].origin_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].length_ms, 7500);

    // Tile 1: Long member part 0
    assert_eq!(plan.tiles[1].day, "20260115");
    assert_eq!(plan.tiles[1].segment_key, "000500_300");
    assert_eq!(plan.tiles[1].parts.len(), 1);
    assert_eq!(plan.tiles[1].parts[0].offset_ms, 150250);
    assert_eq!(plan.tiles[1].parts[0].origin_ms, 0);
    assert_eq!(plan.tiles[1].parts[0].length_ms, 149750);

    // Tile 2: Long member part 1
    assert_eq!(plan.tiles[2].day, "20260115");
    assert_eq!(plan.tiles[2].segment_key, "001000_300");
    assert_eq!(plan.tiles[2].parts.len(), 1);
    assert_eq!(plan.tiles[2].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[2].parts[0].origin_ms, 149750);
    assert_eq!(plan.tiles[2].parts[0].length_ms, 250250);
}

#[test]
fn settled_fold_and_unsettled_folds() {
    // Settled Fold:
    // America/New_York 2026-11-01T01:30:00.000 local, duration 10_000 ms, owner America/New_York.
    // Key 013000_300, offset 0, origin 0, length 10000.
    // Canonical occurrence: local|America/New_York|2026-11-01T01:30:00.000
    let fold_member = AudioArchiveMemberInput {
        original_relative_name: "archive/fold.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(30),
        duration: DurationInput::Milliseconds(10_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-11-01T01:30:00.000".to_owned()),
            source_zone: Some("America/New_York".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };

    let plan_ny =
        plan_audio_archive(std::slice::from_ref(&fold_member), NY, default_settings()).unwrap();
    assert_eq!(plan_ny.members[0].disposition, MemberDisposition::Included);
    assert_eq!(
        plan_ny.members[0].identity.occurrence,
        "local|America/New_York|2026-11-01T01:30:00.000"
    );
    assert_eq!(plan_ny.tiles.len(), 1);
    assert_eq!(plan_ny.tiles[0].day, "20261101");
    assert_eq!(plan_ny.tiles[0].segment_key, "013000_300");
    assert_eq!(plan_ny.tiles[0].parts[0].offset_ms, 0);
    assert_eq!(plan_ny.tiles[0].parts[0].origin_ms, 0);
    assert_eq!(plan_ny.tiles[0].parts[0].length_ms, 10000);

    // Same local start, owner UTC -> UnsettledTime
    let plan_utc = plan_audio_archive(
        std::slice::from_ref(&fold_member),
        UTC_ZONE,
        default_settings(),
    )
    .unwrap();
    assert_eq!(
        plan_utc.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::UnsettledTime)
    );

    // 90-minute fold with America/New_York owner -> UnsettledTime
    let long_fold = AudioArchiveMemberInput {
        original_relative_name: "archive/long_fold.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(31),
        duration: DurationInput::Milliseconds(5_400_000), // 90 min
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-11-01T01:30:00.000".to_owned()),
            source_zone: Some("America/New_York".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan_long_fold = plan_audio_archive(&[long_fold], NY, default_settings()).unwrap();
    assert_eq!(
        plan_long_fold.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::UnsettledTime)
    );

    // Spring forward gap: America/New_York 2026-03-08T02:30:00.000 -> NonexistentLocalTime
    let gap_member = AudioArchiveMemberInput {
        original_relative_name: "archive/gap.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(32),
        duration: DurationInput::Milliseconds(10_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-03-08T02:30:00.000".to_owned()),
            source_zone: Some("America/New_York".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan_gap = plan_audio_archive(&[gap_member], NY, default_settings()).unwrap();
    assert_eq!(
        plan_gap.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::NonexistentLocalTime)
    );
}

#[test]
fn agreeing_and_conflicting_evidence() {
    // Agreeing: instant 2026-01-15T05:07:30.250Z + Auckland 2026-01-15T18:07:30.250
    let agreeing = AudioArchiveMemberInput {
        original_relative_name: "archive/agree.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(40),
        duration: DurationInput::Milliseconds(1000),
        start: MemberStartEvidence {
            instant: Some("2026-01-15T05:07:30.250Z".to_owned()),
            local_civil: Some("2026-01-15T18:07:30.250".to_owned()),
            source_zone: Some("Pacific/Auckland".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan_agree = plan_audio_archive(&[agreeing], NY, default_settings()).unwrap();
    assert_eq!(
        plan_agree.members[0].disposition,
        MemberDisposition::Included
    );

    // Conflicting: instant 2026-01-15T05:07:30.250Z + America/New_York 2026-01-15T18:07:30.250
    let conflicting = AudioArchiveMemberInput {
        original_relative_name: "archive/conflict.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(41),
        duration: DurationInput::Milliseconds(1000),
        start: MemberStartEvidence {
            instant: Some("2026-01-15T05:07:30.250Z".to_owned()),
            local_civil: Some("2026-01-15T18:07:30.250".to_owned()),
            source_zone: Some("America/New_York".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan_conflict = plan_audio_archive(&[conflicting], NY, default_settings()).unwrap();
    assert_eq!(
        plan_conflict.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::ConflictingEvidence)
    );
}

#[test]
fn shared_tile_and_same_start_conflict() {
    // Shared tile: two members in 160000_300 at offsets 0 and 20_000, each 10_000 ms
    let m1 = AudioArchiveMemberInput {
        original_relative_name: "archive/m1.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(50),
        duration: DurationInput::Milliseconds(10_000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let m2 = AudioArchiveMemberInput {
        original_relative_name: "archive/m2.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(51),
        duration: DurationInput::Milliseconds(10_000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:20.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };

    let plan_shared = plan_audio_archive(&[m1, m2], NY, default_settings()).unwrap();
    assert_eq!(plan_shared.tiles.len(), 1);
    assert_eq!(plan_shared.tiles[0].parts.len(), 2);
    assert_eq!(plan_shared.tiles[0].parts[0].offset_ms, 0);
    assert_eq!(plan_shared.tiles[0].parts[1].offset_ms, 20_000);

    // Same-start different hashes: later is IntervalConflict at key 160000_300
    let c1 = AudioArchiveMemberInput {
        original_relative_name: "archive/c1.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(60),
        duration: DurationInput::Milliseconds(10_000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let c2 = AudioArchiveMemberInput {
        original_relative_name: "archive/c2.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(61), // different hash
        duration: DurationInput::Milliseconds(10_000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()), // same start
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };

    let plan_conflict = plan_audio_archive(&[c1, c2], NY, default_settings()).unwrap();
    assert_eq!(
        plan_conflict.members[0].disposition,
        MemberDisposition::Included
    );
    match &plan_conflict.members[1].disposition {
        MemberDisposition::LeftOut(LeftOutReason::IntervalConflict(payload)) => {
            assert_eq!(payload.day, "20260114");
            assert_eq!(payload.segment_key, "160000_300");
            assert_eq!(payload.conflicting_sha256, dummy_hash(60));
        }
        other => panic!("expected IntervalConflict, got {other:?}"),
    }
}

#[test]
fn deduplication_and_naming_rules() {
    // Rename dedupes: same bytes + canonical occurrence, different relative names
    let d1 = AudioArchiveMemberInput {
        original_relative_name: "dir1/clip.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(70),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let d2 = AudioArchiveMemberInput {
        original_relative_name: "dir2/renamed_clip.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(70),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000+00:00".to_owned()), // Z and +00:00 dedupe
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };

    let plan = plan_audio_archive(&[d1, d2], NY, default_settings()).unwrap();
    assert_eq!(plan.members.len(), 2);
    assert_eq!(
        plan.members[1].disposition,
        MemberDisposition::LeftOut(LeftOutReason::DuplicateMember)
    );
    assert_eq!(plan.tiles[0].parts.len(), 1);
    assert_eq!(plan.members[0].original_relative_name, "dir1/clip.ogg"); // Retains first name

    // Equal bytes at different instants stay distinct
    let t1 = AudioArchiveMemberInput {
        original_relative_name: "dir1/clip.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(71),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let t2 = AudioArchiveMemberInput {
        original_relative_name: "dir2/clip.ogg".to_owned(),
        byte_len: 500,
        sha256: dummy_hash(71),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:05:00.000Z".to_owned()), // 5 min later
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let plan_distinct = plan_audio_archive(&[t1, t2], NY, default_settings()).unwrap();
    assert_eq!(plan_distinct.members.len(), 2);
    assert_eq!(plan_distinct.tiles.len(), 2);
}

#[cfg(feature = "full-tests")]
#[test]
fn claim_index_semantics_and_missing_run() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    // find_held_members does not create database
    let plan = plan_audio_archive(
        &[AudioArchiveMemberInput {
            original_relative_name: "clip.ogg".to_owned(),
            byte_len: 500,
            sha256: dummy_hash(80),
            duration: DurationInput::Milliseconds(5000),
            start: MemberStartEvidence {
                instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        }],
        NY,
        default_settings(),
    )
    .unwrap();

    let held = find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT).unwrap();
    assert!(held.is_empty());
    assert!(!claim_db_path(journal).exists());

    // Plant garbage at claim path -> corrupt error
    fs::create_dir_all(journal.join("imports")).unwrap();
    fs::write(claim_db_path(journal), b"not a sqlite db").unwrap();
    let err =
        find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT).unwrap_err();
    assert!(matches!(err, AudioArchiveError::ClaimDatabaseCorrupt(_)));
    fs::remove_file(claim_db_path(journal)).unwrap();

    // Insert claims under run1
    let id1 = plan.members[0].identity.clone();
    insert_tile_claims(
        journal,
        "20260114_100001",
        std::slice::from_ref(&id1),
        DEFAULT_WRITE_BUSY_TIMEOUT,
    )
    .unwrap();

    // With run directory present, run2 sees it held
    let run1_dir = journal.join("imports").join("20260114_100001");
    save_audio_archive_record(
        journal,
        "20260114_100001",
        &super::record::plan_to_record(&plan),
    )
    .unwrap();
    let held = find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT).unwrap();
    assert!(held.contains(&id1));

    // Remove run1 directory -> claim does not block and no tombstone is written
    fs::remove_dir_all(&run1_dir).unwrap();
    let held_after_remove =
        find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT).unwrap();
    assert!(held_after_remove.is_empty());
    assert!(!run1_dir.exists());

    // Insert under run2 updates claim row to run2 without error
    insert_tile_claims(
        journal,
        "20260114_100002",
        std::slice::from_ref(&id1),
        DEFAULT_WRITE_BUSY_TIMEOUT,
    )
    .unwrap();
}

#[cfg(feature = "full-tests")]
#[test]
fn publication_and_fault_seams() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let member_bytes_data = b"synthetic ogg audio bytes".to_vec();
    let byte_len = member_bytes_data.len() as u64;
    let sha256 = format!("{:x}", Sha256::digest(&member_bytes_data));

    let member = AudioArchiveMemberInput {
        original_relative_name: "archive/clip.ogg".to_owned(),
        byte_len,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(7500),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.500Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };

    let mut bytes_map = HashMap::new();
    bytes_map.insert(sha256.clone(), member_bytes_data.clone());

    // Seam 1: AfterRecordFirstSaved
    let seams_fault1 = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::AfterRecordFirstSaved),
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };

    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_fault1),
    );
    assert!(matches!(
        res,
        Err(AudioArchiveError::Fault(
            ArchiveFault::AfterRecordFirstSaved
        ))
    ));

    // Verify recovery on clean publish
    let seams_clean = AudioArchivePublishSeams {
        fault: None,
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };
    let clean_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_clean),
    )
    .unwrap();

    assert_eq!(clean_res.tiles_saved, 1);
    assert_eq!(clean_res.parts_saved, 1);

    // Frontier test on published run
    let frontier = update_archive_frontier(journal, &clean_res.import_id).unwrap();
    assert_eq!(frontier.published_tiles.len(), 1);
    assert_eq!(frontier.pending_tiles.len(), 1);
    assert_eq!(frontier.unavailable_parts.len(), 0);

    // Remove chronicle raw -> frontier reports SavedButUnavailable
    let raw_path = journal
        .join("chronicle")
        .join("20260114")
        .join("import.clips")
        .join("160000_300")
        .join(&clean_res.record.parts[0].stored_name);
    fs::remove_file(&raw_path).unwrap();

    let frontier_after_removal = update_archive_frontier(journal, &clean_res.import_id).unwrap();
    assert_eq!(frontier_after_removal.unavailable_parts.len(), 1);
    assert!(frontier_after_removal.pending_tiles.is_empty());
    let status = ArchiveStore::open(journal, &clean_res.import_id, false)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(status.unavailable_parts, 1);
    assert_eq!(status.processed_parts, 0);
}

#[test]
fn negative_offset_instant_dedupes_with_z_and_auckland_original_time() {
    let m1 = AudioArchiveMemberInput {
        original_relative_name: "a/one.ogg".to_owned(),
        byte_len: 1024,
        sha256: dummy_hash(20),
        duration: DurationInput::Milliseconds(7_500),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T16:00:00.500-05:00".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let m2 = AudioArchiveMemberInput {
        original_relative_name: "b/two.ogg".to_owned(),
        byte_len: 1024,
        sha256: dummy_hash(20),
        duration: DurationInput::Milliseconds(7_500),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.500Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let short_auckland = AudioArchiveMemberInput {
        original_relative_name: "archive/short.ogg".to_owned(),
        byte_len: 1024,
        sha256: dummy_hash(21),
        duration: DurationInput::Milliseconds(7_500),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-01-15T10:00:00.500".to_owned()),
            source_zone: Some("Pacific/Auckland".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };

    let plan_dedupe = plan_audio_archive(&[m1, m2], NY, default_settings()).unwrap();
    assert_eq!(plan_dedupe.members.len(), 2);
    assert_eq!(
        plan_dedupe.members[1].disposition,
        MemberDisposition::LeftOut(LeftOutReason::DuplicateMember)
    );
    assert_eq!(
        plan_dedupe.members[0].identity.occurrence,
        "utc|1768424400500"
    );
    assert_eq!(plan_dedupe.members[0].original_relative_name, "a/one.ogg");
    assert_eq!(
        plan_dedupe.members[0].disposition,
        MemberDisposition::Included
    );

    let plan_auckland = plan_audio_archive(&[short_auckland], NY, default_settings()).unwrap();
    assert_eq!(
        plan_auckland.members[0].identity.occurrence,
        "utc|1768424400500"
    );
    assert_eq!(
        plan_auckland.tiles[0].parts[0].original_time_ms,
        Some(1_768_424_400_500)
    );
}

#[test]
fn qualifier_audio_refused() {
    let settings = AudioArchiveSettings {
        qualifier: "audio".to_owned(),
        archive_identity: "test_archive".to_owned(),
    };
    let res = plan_audio_archive(&[], NY, settings);
    assert!(matches!(res, Err(AudioArchiveError::InvalidStreamName(_))));
}

#[test]
fn distinct_stored_names_and_long_member_second_part() {
    let m1 = AudioArchiveMemberInput {
        original_relative_name: "dir_a/clip.ogg".to_owned(),
        byte_len: 100,
        sha256: dummy_hash(30),
        duration: DurationInput::Milliseconds(5_000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let m2 = AudioArchiveMemberInput {
        original_relative_name: "dir_b/clip.ogg".to_owned(),
        byte_len: 200,
        sha256: dummy_hash(31),
        duration: DurationInput::Milliseconds(5_000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:05:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let long_member = AudioArchiveMemberInput {
        original_relative_name: "archive/long.ogg".to_owned(),
        byte_len: 2048,
        sha256: dummy_hash(32),
        duration: DurationInput::Milliseconds(400_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-01-15T18:07:30.250".to_owned()),
            source_zone: Some("Pacific/Auckland".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };

    let plan = plan_audio_archive(&[m1, m2, long_member], NY, default_settings()).unwrap();
    let name1 = &plan.tiles[0].parts[0].stored_name;
    let name2 = &plan.tiles[1].parts[0].stored_name;
    assert_ne!(name1, name2);

    let p1 = &plan.tiles[3].parts[0];
    assert_eq!(p1.offset_ms, 0);
    assert_eq!(p1.origin_ms, 149750);
    assert_eq!(p1.length_ms, 250250);
    assert!(!p1.stored_name.contains("000500"));
    assert!(!p1.stored_name.contains("001000"));
}

#[cfg(feature = "full-tests")]
#[test]
fn malformed_manifest_ignored_by_claims() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let legacy_import = journal.join("imports").join("20260101_000000");
    fs::create_dir_all(&legacy_import).unwrap();
    fs::write(legacy_import.join("manifest.json"), b"{").unwrap();

    let plan = plan_audio_archive(
        &[AudioArchiveMemberInput {
            original_relative_name: "clip.ogg".to_owned(),
            byte_len: 500,
            sha256: dummy_hash(40),
            duration: DurationInput::Milliseconds(5000),
            start: MemberStartEvidence {
                instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        }],
        NY,
        default_settings(),
    )
    .unwrap();

    let res = find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT);
    assert!(res.is_ok());
    assert!(res.unwrap().is_empty());
}

#[cfg(feature = "full-tests")]
#[test]
fn each_archive_fault_and_resume_publishes_raw() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let raw_bytes = b"fault_testing_audio_content".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let byte_len = raw_bytes.len() as u64;

    let member = AudioArchiveMemberInput {
        original_relative_name: "test.ogg".to_owned(),
        byte_len,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut bytes_map = HashMap::new();
    bytes_map.insert(sha256.clone(), raw_bytes.clone());

    let faults = [
        ArchiveFault::AfterRecordFirstSaved,
        ArchiveFault::AfterTileBytesStaged,
        ArchiveFault::BeforeTilePublish,
        ArchiveFault::AfterPublishBeforeSavedMark,
        ArchiveFault::AfterSavedMarkBeforeEmit,
    ];

    for fault in faults {
        let t = TempDir::new().unwrap();
        let seams = AudioArchivePublishSeams {
            fault: Some(fault),
            emit_observed: None,
            heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
            progress: None,
        };
        let res = publish_audio_archive(
            t.path(),
            AudioArchivePublishOptions {
                import_id: None,
                members: vec![member.clone()],
                materializer: &bytes_map,
                owner_zone: NY,
                settings: default_settings(),
                source_timestamp: None,
            },
            Some(seams),
        );
        assert!(matches!(res, Err(AudioArchiveError::Fault(f)) if f == fault));
    }

    let seams_claim = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::AfterClaimAndIntent),
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };
    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_claim),
    );
    assert!(matches!(
        res,
        Err(AudioArchiveError::Fault(ArchiveFault::AfterClaimAndIntent))
    ));

    let imports_dir = journal.join("imports");
    let run_id = fs::read_dir(&imports_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();

    let seams_clean = AudioArchivePublishSeams {
        fault: None,
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };
    let resume_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&run_id),
            members: vec![member.clone()],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_clean),
    )
    .unwrap();

    assert_eq!(resume_res.tiles_saved, 1);
    assert_eq!(resume_res.parts_saved, 1);

    let part = &resume_res.record.parts[0];
    let raw_file = journal
        .join("chronicle")
        .join(&part.day)
        .join("import.clips")
        .join(&part.segment_key)
        .join(&part.stored_name);
    assert!(raw_file.is_file());

    let claim_db = claim_db_path(journal);
    let conn = rusqlite::Connection::open(&claim_db).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM audio_archive_claim", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[cfg(feature = "full-tests")]
#[test]
fn between_multi_tile_parts_fault_semantics() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let raw_bytes = vec![42u8; 2048];
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));

    let long_member = AudioArchiveMemberInput {
        original_relative_name: "archive/long.ogg".to_owned(),
        byte_len: 2048,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(400_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-01-15T18:07:30.250".to_owned()),
            source_zone: Some("Pacific/Auckland".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256.clone(), raw_bytes);

    let seams = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::BetweenMultiTileParts),
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };

    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![long_member],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams),
    );
    assert!(matches!(
        res,
        Err(AudioArchiveError::Fault(
            ArchiveFault::BetweenMultiTileParts
        ))
    ));

    let imports_dir = journal.join("imports");
    let run_id = fs::read_dir(&imports_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();

    let record = read_audio_archive(journal, &run_id)
        .unwrap()
        .expect("record exists");
    assert_eq!(record.parts.len(), 2);

    let part0 = &record.parts[0];
    assert_eq!(part0.outcome, PartOutcome::Saved);
    let tile0_placement = journal
        .join("chronicle")
        .join(&part0.day)
        .join("import.clips")
        .join(&part0.segment_key)
        .join(AUDIO_ARCHIVE_PLACEMENT_FILENAME);
    let tile0_raw = journal
        .join("chronicle")
        .join(&part0.day)
        .join("import.clips")
        .join(&part0.segment_key)
        .join(&part0.stored_name);
    assert!(tile0_placement.is_file());
    assert!(tile0_raw.is_file());

    let part1 = &record.parts[1];
    assert_ne!(part1.outcome, PartOutcome::Saved);
    let tile1_placement = journal
        .join("chronicle")
        .join(&part1.day)
        .join("import.clips")
        .join(&part1.segment_key)
        .join(AUDIO_ARCHIVE_PLACEMENT_FILENAME);
    assert!(!tile1_placement.exists());

    let conn = rusqlite::Connection::open(claim_db_path(journal)).unwrap();
    let count: i64 = conn
        .query_row("SELECT count(*) FROM audio_archive_claim", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1);
}

#[cfg(feature = "full-tests")]
#[test]
fn resume_settings_mismatch_leaves_record_unchanged() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let raw_bytes = b"mismatch_audio_test".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let member = AudioArchiveMemberInput {
        original_relative_name: "test.ogg".to_owned(),
        byte_len: raw_bytes.len() as u64,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256, raw_bytes);

    let seams_fault = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::AfterRecordFirstSaved),
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };
    let _ = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_fault),
    );

    let imports_dir = journal.join("imports");
    let run_id = fs::read_dir(&imports_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.file_type().map(|ft| ft.is_dir()).unwrap_or(false))
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();

    let record_path = journal
        .join("imports")
        .join(&run_id)
        .join(super::record::AUDIO_ARCHIVE_RECORD_FILENAME);
    let original_bytes = fs::read(&record_path).unwrap();

    let mismatched_settings = AudioArchiveSettings {
        qualifier: "clips".to_owned(),
        archive_identity: "completely_different_identity".to_owned(),
    };
    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&run_id),
            members: vec![member],
            materializer: &map,
            owner_zone: NY,
            settings: mismatched_settings,
            source_timestamp: None,
        },
        None,
    );
    assert!(matches!(
        res,
        Err(AudioArchiveError::SettingsMismatch { .. })
    ));

    let bytes_after = fs::read(&record_path).unwrap();
    assert_eq!(original_bytes, bytes_after);
}

#[cfg(feature = "full-tests")]
#[test]
fn delete_chronicle_raw_and_resume_does_not_recreate() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let raw_bytes = b"delete_test_audio".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let member = AudioArchiveMemberInput {
        original_relative_name: "test.ogg".to_owned(),
        byte_len: raw_bytes.len() as u64,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256, raw_bytes);

    let clean = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();

    let part = &clean.record.parts[0];
    let raw_path = journal
        .join("chronicle")
        .join(&part.day)
        .join("import.clips")
        .join(&part.segment_key)
        .join(&part.stored_name);
    assert!(raw_path.is_file());

    fs::remove_file(&raw_path).unwrap();
    assert!(!raw_path.exists());

    let resume_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&clean.import_id),
            members: vec![member],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();

    assert_eq!(resume_res.tiles_saved, 0);
    assert_eq!(resume_res.parts_saved, 0);
    assert!(!raw_path.exists());

    let frontier = update_archive_frontier(journal, &clean.import_id).unwrap();
    assert!(frontier.pending_tiles.is_empty());
    let status = ArchiveStore::open(journal, &clean.import_id, false)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(status.unavailable_parts, 1);
    assert_eq!(status.processed_parts, 0);
}

#[cfg(feature = "full-tests")]
#[test]
fn foreign_file_in_segment_refused_no_key_advance() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let clip_dir = journal
        .join("chronicle")
        .join("20260114")
        .join("import.clips")
        .join("160000_300");
    fs::create_dir_all(&clip_dir).unwrap();
    let foreign_path = clip_dir.join("foreign.txt");
    fs::write(&foreign_path, b"foreign content").unwrap();

    let phone_dir = journal
        .join("chronicle")
        .join("20260114")
        .join("device.phone")
        .join("160000_300");
    fs::create_dir_all(&phone_dir).unwrap();
    let phone_path = phone_dir.join("phone.txt");
    fs::write(&phone_path, b"phone data").unwrap();

    let raw_bytes = b"planned_member_bytes".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let member = AudioArchiveMemberInput {
        original_relative_name: "test.ogg".to_owned(),
        byte_len: raw_bytes.len() as u64,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256, raw_bytes);

    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    );
    let result = res.unwrap();
    assert!(matches!(
        result.record.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::PublishedTile { .. })
    ));
    assert_eq!(result.parts_saved, 0);

    assert_eq!(fs::read(&foreign_path).unwrap(), b"foreign content");
    assert_eq!(fs::read(&phone_path).unwrap(), b"phone data");
    let parent = clip_dir.parent().unwrap();
    let entries: Vec<_> = fs::read_dir(parent)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), "160000_300");
}

#[cfg(feature = "full-tests")]
#[test]
fn processing_frontier_retires_only_the_acknowledged_member_part() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let sources = [
        storage_member(80, "2026-01-14T21:00:00Z", 7500),
        storage_member(81, "2026-01-14T21:00:12.500Z", 720),
        storage_member(82, "2026-01-15T23:58:17.123-05:00", 315485),
    ];
    let materials: HashMap<_, _> = sources
        .iter()
        .map(|(member, bytes)| (member.sha256.clone(), bytes.clone()))
        .collect();
    let members: Vec<_> = sources.iter().map(|(member, _)| member.clone()).collect();
    let result = storage_publish(journal, None, &members, &materials, None).unwrap();
    assert_eq!(result.record.parts.len(), 4);
    let first = &result.record.parts[0];
    finish_archive_part(
        journal,
        &result.import_id,
        &first.day,
        &first.segment_key,
        &first.stored_name,
        false,
    )
    .unwrap();
    let long_first = result
        .record
        .parts
        .iter()
        .find(|part| part.member_sha256 == members[2].sha256 && part.origin_ms == 0)
        .unwrap();
    finish_archive_part(
        journal,
        &result.import_id,
        &long_first.day,
        &long_first.segment_key,
        &long_first.stored_name,
        false,
    )
    .unwrap();
    let batch = update_pending_archive_tiles(journal, 64).unwrap();
    assert!(batch.errors.is_empty());
    assert_eq!(
        batch
            .tiles
            .iter()
            .map(|tile| tile.parts.len())
            .sum::<usize>(),
        2
    );
    let missing = result
        .record
        .parts
        .iter()
        .find(|part| part.member_sha256 == members[1].sha256)
        .unwrap();
    fs::remove_file(
        journal
            .join("chronicle")
            .join(&missing.day)
            .join(&result.record.settings.stream)
            .join(&missing.segment_key)
            .join(&missing.stored_name),
    )
    .unwrap();
    let batch = update_pending_archive_tiles(journal, 64).unwrap();
    assert!(batch.errors.is_empty());
    assert_eq!(batch.tiles.len(), 1);
    assert_eq!(batch.tiles[0].parts.len(), 1);
    let last = &batch.tiles[0];
    finish_archive_part(
        journal,
        &result.import_id,
        &last.day,
        &last.segment_key,
        &last.parts[0].stored_name,
        false,
    )
    .unwrap();
    assert!(
        update_pending_archive_tiles(journal, 64)
            .unwrap()
            .tiles
            .is_empty()
    );
    let status = ArchiveStore::open(journal, &result.import_id, false)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(
        (
            status.saved_parts,
            status.saved_members,
            status.processed_parts,
            status.unavailable_parts,
            status.pending_processing
        ),
        (4, 3, 3, 1, 0)
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn emit_seam_invoked_once_per_finished_tile() {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();

    let emit_fn: EmitObservedFn = Box::new(move |_day, _key, _stream| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    });

    let raw_bytes = b"emit_seam_audio_bytes".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let member = AudioArchiveMemberInput {
        original_relative_name: "test.ogg".to_owned(),
        byte_len: raw_bytes.len() as u64,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256, raw_bytes);

    let seams_fault = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::AfterClaimAndIntent),
        emit_observed: Some(emit_fn),
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };

    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_fault),
    );
    assert!(res.is_err());
    assert_eq!(count.load(Ordering::SeqCst), 0);

    let clean_count = Arc::new(AtomicUsize::new(0));
    let clean_count_clone = clean_count.clone();
    let emit_clean: EmitObservedFn = Box::new(move |_day, _key, _stream| {
        clean_count_clone.fetch_add(1, Ordering::SeqCst);
    });

    let seams_clean = AudioArchivePublishSeams {
        fault: None,
        emit_observed: Some(emit_clean),
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };

    let temp_clean = TempDir::new().unwrap();
    let clean_res = publish_audio_archive(
        temp_clean.path(),
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_clean),
    )
    .unwrap();
    assert_eq!(clean_res.tiles_saved, 1);
    assert_eq!(clean_count.load(Ordering::SeqCst), 1);
}

#[cfg(feature = "full-tests")]
#[test]
fn claim_contention_zero_timeout_returns_busy() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let id1 = MemberIdentity::new(100, dummy_hash(1), "utc|1768424400500");
    insert_tile_claims(journal, "run_1", &[id1], Duration::from_millis(2000)).unwrap();

    let path = claim_db_path(journal);
    let blocker_conn = rusqlite::Connection::open(&path).unwrap();
    blocker_conn.execute_batch("BEGIN IMMEDIATE;").unwrap();

    let id2 = MemberIdentity::new(200, dummy_hash(2), "utc|1768424400500");
    let res = insert_tile_claims(journal, "run_2", &[id2], Duration::ZERO);
    assert!(matches!(res, Err(AudioArchiveError::ClaimDatabaseBusy(_))));

    blocker_conn.execute_batch("COMMIT;").unwrap();
}

#[cfg(feature = "full-tests")]
#[test]
fn before_tile_publish_resume_publishes_one_complete_tile() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let member_bytes_data = b"synthetic test audio raw bytes".to_vec();
    let byte_len = member_bytes_data.len() as u64;
    let sha256 = format!("{:x}", Sha256::digest(&member_bytes_data));

    let member = AudioArchiveMemberInput {
        original_relative_name: "archive/clip.ogg".to_owned(),
        byte_len,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(7500),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.500Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };

    let mut bytes_map = HashMap::new();
    bytes_map.insert(sha256.clone(), member_bytes_data.clone());

    let seams_fault = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::BeforeTilePublish),
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };

    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_fault),
    );
    assert!(matches!(
        res,
        Err(AudioArchiveError::Fault(ArchiveFault::BeforeTilePublish))
    ));

    // Find the import_id created by this attempt
    let import_id = fs::read_dir(journal.join("imports"))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.path().is_dir())
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();

    let record = read_audio_archive(journal, &import_id).unwrap().unwrap();
    let part = &record.parts[0];
    let segment_dir = journal
        .join("chronicle")
        .join(&part.day)
        .join("import.clips")
        .join(&part.segment_key);
    let partial_path = journal
        .join("imports")
        .join(&import_id)
        .join("staged")
        .join(&part.day)
        .join(&part.segment_key)
        .join(&part.stored_name);
    let raw_path = segment_dir.join(&part.stored_name);

    assert!(partial_path.is_file());
    assert!(!segment_dir.exists());
    assert!(!raw_path.exists());

    // Resume with same import_id and no fault
    let seams_clean = AudioArchivePublishSeams {
        fault: None,
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };
    let clean_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&import_id),
            members: vec![member],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_clean),
    )
    .unwrap();

    assert_eq!(clean_res.tiles_saved, 1);
    assert_eq!(clean_res.parts_saved, 1);
    assert!(raw_path.is_file());
    assert!(!partial_path.exists());

    // Exactly one claim row in claim db
    let conn = rusqlite::Connection::open(claim_db_path(journal)).unwrap();
    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_archive_claim WHERE import_id = ?",
            [&import_id],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[cfg(feature = "full-tests")]
#[test]
fn after_publish_before_saved_mark_delete_raw_resume() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let member_bytes_data = b"synthetic test audio bytes for unavailable".to_vec();
    let byte_len = member_bytes_data.len() as u64;
    let sha256 = format!("{:x}", Sha256::digest(&member_bytes_data));

    let member = AudioArchiveMemberInput {
        original_relative_name: "archive/clip.ogg".to_owned(),
        byte_len,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(7500),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.500Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };

    let mut bytes_map = HashMap::new();
    bytes_map.insert(sha256.clone(), member_bytes_data.clone());

    let seams_fault = AudioArchivePublishSeams {
        fault: Some(ArchiveFault::AfterPublishBeforeSavedMark),
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };

    let res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_fault),
    );
    assert!(matches!(
        res,
        Err(AudioArchiveError::Fault(
            ArchiveFault::AfterPublishBeforeSavedMark
        ))
    ));

    // Find the import_id
    let import_id = fs::read_dir(journal.join("imports"))
        .unwrap()
        .filter_map(|e| e.ok())
        .find(|e| e.path().is_dir())
        .unwrap()
        .file_name()
        .into_string()
        .unwrap();

    let record = read_audio_archive(journal, &import_id).unwrap().unwrap();
    let part = &record.parts[0];
    let segment_dir = journal
        .join("chronicle")
        .join(&part.day)
        .join("import.clips")
        .join(&part.segment_key);
    let raw_path = segment_dir.join(&part.stored_name);
    assert!(raw_path.is_file());

    // Delete the chronicle raw
    fs::remove_file(&raw_path).unwrap();
    assert!(!raw_path.exists());

    // Resume same id with no fault
    let seams_clean = AudioArchivePublishSeams {
        fault: None,
        emit_observed: None,
        heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
        progress: None,
    };
    let clean_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&import_id),
            members: vec![member],
            materializer: &bytes_map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(seams_clean),
    )
    .unwrap();

    assert_eq!(clean_res.tiles_saved, 1);
    assert_eq!(clean_res.parts_saved, 1);
    // The raw stays absent and was not recreated
    assert!(!raw_path.exists());
    // The saved outcome is SavedButUnavailable
    assert_eq!(
        clean_res.record.parts[0].outcome,
        PartOutcome::SavedButUnavailable
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn corrupt_or_missing_run_record_is_not_treated_as_an_absent_run() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (member, _) = storage_member(70, "2026-01-14T21:00:00Z", 5000);
    let plan = plan_audio_archive(&[member], NY, default_settings()).unwrap();
    let id = "20260114_100003";
    save_audio_archive_record(journal, id, &super::record::plan_to_record(&plan)).unwrap();
    insert_tile_claims(
        journal,
        id,
        &[plan.members[0].identity.clone()],
        DEFAULT_WRITE_BUSY_TIMEOUT,
    )
    .unwrap();
    let path = super::record::audio_archive_record_path(journal, id);
    fs::write(&path, b"invalid database").unwrap();
    assert!(matches!(
        find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT),
        Err(AudioArchiveError::ClaimDatabaseCorrupt(_))
    ));
    fs::remove_file(path).unwrap();
    assert!(matches!(
        find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT),
        Err(AudioArchiveError::ClaimDatabaseCorrupt(_))
    ));
    fs::remove_dir_all(journal.join("imports").join(id)).unwrap();
    assert!(
        find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn dst_fixture_fall_back_boundary() {
    let m = AudioArchiveMemberInput {
        original_relative_name: "fall_back_boundary.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(31),
        duration: DurationInput::Milliseconds(20_000),
        start: MemberStartEvidence {
            instant: Some("2026-11-01T01:59:50-04:00".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(&[m], NY, default_settings()).unwrap();
    assert_eq!(plan.members[0].disposition, MemberDisposition::Included);
    assert_eq!(plan.tiles.len(), 2);

    let t0 = plan
        .tiles
        .iter()
        .find(|t| t.segment_key == "015500_300")
        .expect("015500_300");
    assert_eq!(t0.day, "20261101");
    assert_eq!(t0.segment_key, "015500_300");
    assert_eq!(t0.parts.len(), 1);
    assert_eq!(t0.parts[0].offset_ms, 290_000);
    assert_eq!(t0.parts[0].origin_ms, 0);
    assert_eq!(t0.parts[0].length_ms, 10_000);
    assert_eq!(t0.parts[0].offset_ms + t0.parts[0].length_ms, 300_000);

    let t1 = plan
        .tiles
        .iter()
        .find(|t| t.segment_key == "010000_300")
        .expect("010000_300");
    assert_eq!(t1.day, "20261101");
    assert_eq!(t1.segment_key, "010000_300");
    assert_eq!(t1.parts.len(), 1);
    assert_eq!(t1.parts[0].offset_ms, 0);
    assert_eq!(t1.parts[0].origin_ms, 10_000);
    assert_eq!(t1.parts[0].length_ms, 10_000);
}

#[test]
fn dst_fixture_fall_back_crossing_unique() {
    let m = AudioArchiveMemberInput {
        original_relative_name: "fall_back_crossing.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(32),
        duration: DurationInput::Milliseconds(360_000),
        start: MemberStartEvidence {
            instant: Some("2026-11-01T01:30:00-04:00".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(&[m], NY, default_settings()).unwrap();
    assert_eq!(plan.members[0].disposition, MemberDisposition::Included);
    assert_eq!(plan.tiles.len(), 2);

    assert_eq!(plan.tiles[0].segment_key, "013000_300");
    assert_eq!(plan.tiles[0].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].origin_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[1].segment_key, "013500_300");
    assert_eq!(plan.tiles[1].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[1].parts[0].origin_ms, 300_000);
    assert_eq!(plan.tiles[1].parts[0].length_ms, 60_000);
}

#[test]
fn ambiguous_long_civil_requires_an_instant() {
    let m = AudioArchiveMemberInput {
        original_relative_name: "ambiguous_settled.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(33),
        duration: DurationInput::Milliseconds(360_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-11-01T01:30:00.000".to_owned()),
            source_zone: Some("America/New_York".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(&[m], NY, default_settings()).unwrap();
    assert_eq!(
        plan.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::UnsettledTime)
    );
    assert!(plan.tiles.is_empty());
}

#[test]
fn dst_fixture_spring_forward_gap_crossing() {
    let m = AudioArchiveMemberInput {
        original_relative_name: "spring_forward_1200s.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(34),
        duration: DurationInput::Milliseconds(1_200_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-03-08T01:50:00.000".to_owned()),
            source_zone: Some("America/New_York".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(&[m], NY, default_settings()).unwrap();
    assert_eq!(plan.members[0].disposition, MemberDisposition::Included);
    assert_eq!(plan.tiles.len(), 4);

    assert_eq!(plan.tiles[0].segment_key, "015000_300");
    assert_eq!(plan.tiles[0].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].origin_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[1].segment_key, "015500_300");
    assert_eq!(plan.tiles[1].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[1].parts[0].origin_ms, 300_000);
    assert_eq!(plan.tiles[1].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[2].segment_key, "030000_300");
    assert_eq!(plan.tiles[2].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[2].parts[0].origin_ms, 600_000);
    assert_eq!(plan.tiles[2].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[3].segment_key, "030500_300");
    assert_eq!(plan.tiles[3].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[3].parts[0].origin_ms, 900_000);
    assert_eq!(plan.tiles[3].parts[0].length_ms, 300_000);

    for tile in &plan.tiles {
        assert!(!tile.segment_key.starts_with("02"));
        for part in &tile.parts {
            assert!(part.offset_ms >= 0);
            assert!(part.length_ms > 0);
            assert!(part.offset_ms + part.length_ms <= 300_000);
        }
    }
}

#[test]
fn utc_spring_forward_date_does_not_merge_remainder() {
    let m = AudioArchiveMemberInput {
        original_relative_name: "utc_spring_forward_date_1200s.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(36),
        duration: DurationInput::Milliseconds(1_200_000),
        start: MemberStartEvidence {
            instant: None,
            local_civil: Some("2026-03-08T02:50:00.000".to_owned()),
            source_zone: Some("UTC".to_owned()),
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(&[m], UTC_ZONE, default_settings()).unwrap();
    assert_eq!(plan.members[0].disposition, MemberDisposition::Included);
    assert_eq!(plan.tiles.len(), 4);

    assert_eq!(plan.tiles[0].segment_key, "025000_300");
    assert_eq!(plan.tiles[0].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].origin_ms, 0);
    assert_eq!(plan.tiles[0].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[1].segment_key, "025500_300");
    assert_eq!(plan.tiles[1].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[1].parts[0].origin_ms, 300_000);
    assert_eq!(plan.tiles[1].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[2].segment_key, "030000_300");
    assert_eq!(plan.tiles[2].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[2].parts[0].origin_ms, 600_000);
    assert_eq!(plan.tiles[2].parts[0].length_ms, 300_000);

    assert_eq!(plan.tiles[3].segment_key, "030500_300");
    assert_eq!(plan.tiles[3].parts[0].offset_ms, 0);
    assert_eq!(plan.tiles[3].parts[0].origin_ms, 900_000);
    assert_eq!(plan.tiles[3].parts[0].length_ms, 300_000);

    for tile in &plan.tiles {
        for part in &tile.parts {
            assert!(part.offset_ms >= 0);
            assert!(part.length_ms > 0);
            assert!(part.offset_ms + part.length_ms <= 300_000);
        }
    }
}

#[test]
fn duration_i64_max_left_out_unsettled_time() {
    let m = AudioArchiveMemberInput {
        original_relative_name: "overflow_i64_max.ogg".to_owned(),
        byte_len: 1000,
        sha256: dummy_hash(35),
        duration: DurationInput::Milliseconds(i64::MAX),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let plan = plan_audio_archive(&[m], NY, default_settings()).unwrap();
    assert_eq!(
        plan.members[0].disposition,
        MemberDisposition::LeftOut(LeftOutReason::UnsettledTime)
    );
    assert_eq!(plan.tiles.len(), 0);
}

#[cfg(feature = "full-tests")]
#[test]
fn placement_different_offset_refuses_chronicle_segment() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let raw_bytes = b"different_offset_audio".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let member = AudioArchiveMemberInput {
        original_relative_name: "offset_test.ogg".to_owned(),
        byte_len: raw_bytes.len() as u64,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256.clone(), raw_bytes.clone());

    let clean = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();

    let part = &clean.record.parts[0];
    let seg_dir = journal
        .join("chronicle")
        .join(&part.day)
        .join("import.clips")
        .join(&part.segment_key);
    let placement_path = seg_dir.join(AUDIO_ARCHIVE_PLACEMENT_FILENAME);

    let mismatched_placement = solstone_core_format::audio_archive::AudioArchivePlacement {
        version: 1,
        parts: vec![
            solstone_core_format::audio_archive::AudioArchivePlacementPart {
                stored_name: part.stored_name.clone(),
                original_occurrence: part.member_occurrence.clone(),
                original_time_ms: None,
                origin_ms: part.origin_ms,
                offset_ms: part.offset_ms + 100,
                length_ms: part.length_ms,
            },
        ],
    };
    fs::write(
        &placement_path,
        serde_json::to_vec_pretty(&mismatched_placement).unwrap(),
    )
    .unwrap();

    let resume_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&clean.import_id),
            members: vec![member],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    );
    assert!(matches!(
        resume_res,
        Err(AudioArchiveError::ChronicleSegmentRefused(_))
    ));
}

#[cfg(feature = "full-tests")]
#[test]
fn publication_candidate_file_does_not_refuse_and_resume_does_not_recreate() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let raw_bytes = b"pub_candidate_probe_audio".to_vec();
    let sha256 = format!("{:x}", Sha256::digest(&raw_bytes));
    let member = AudioArchiveMemberInput {
        original_relative_name: "pub_candidate.ogg".to_owned(),
        byte_len: raw_bytes.len() as u64,
        sha256: sha256.clone(),
        duration: DurationInput::Milliseconds(5000),
        start: MemberStartEvidence {
            instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
            local_civil: None,
            source_zone: None,
            ..Default::default()
        },
        run_note: None,
    };
    let mut map = HashMap::new();
    map.insert(sha256, raw_bytes);

    let clean = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: None,
            members: vec![member.clone()],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();

    let part = &clean.record.parts[0];
    let seg_dir = journal
        .join("chronicle")
        .join(&part.day)
        .join("import.clips")
        .join(&part.segment_key);
    let raw_path = seg_dir.join(&part.stored_name);
    assert!(raw_path.is_file());

    fs::remove_file(&raw_path).unwrap();

    let probe_file = seg_dir.join(".tmp_resume_probe.tmp");
    fs::write(&probe_file, b"probe candidate data").unwrap();

    let resume_res = publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: Some(&clean.import_id),
            members: vec![member],
            materializer: &map,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        None,
    )
    .unwrap();

    assert_eq!(resume_res.tiles_saved, 0);
    assert_eq!(resume_res.parts_saved, 0);
    assert!(!raw_path.exists());
    assert_eq!(
        resume_res.record.parts[0].outcome,
        PartOutcome::SavedButUnavailable
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn interrupted_pre_intent_stage_can_be_restaged_without_publishing_partial_bytes() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (member, bytes) = storage_member(71, "2026-01-14T21:00:00Z", 5000);
    let materials = HashMap::from([(member.sha256.clone(), bytes.clone())]);
    assert!(matches!(
        storage_publish(
            journal,
            None,
            std::slice::from_ref(&member),
            &materials,
            Some(ArchiveFault::AfterTileBytesStaged)
        ),
        Err(AudioArchiveError::Fault(ArchiveFault::AfterTileBytesStaged))
    ));
    let id = storage_run(journal);
    let record = read_audio_archive(journal, &id).unwrap().unwrap();
    let part = &record.parts[0];
    let staged = journal
        .join("imports")
        .join(&id)
        .join("staged")
        .join(&part.day)
        .join(&part.segment_key)
        .join(&part.stored_name);
    let destination = journal
        .join("chronicle")
        .join(&part.day)
        .join(&record.settings.stream)
        .join(&part.segment_key);
    assert!(!destination.exists());
    fs::write(staged, b"partial").unwrap();
    let resumed = storage_publish(journal, Some(&id), &[member], &materials, None).unwrap();
    assert_eq!(resumed.parts_saved, 1);
    assert_eq!(
        fs::read(destination.join(&part.stored_name)).unwrap(),
        bytes
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn find_held_members_zero_timeout_under_immediate_returns_busy() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let plan = plan_audio_archive(
        &[AudioArchiveMemberInput {
            original_relative_name: "busy_test.ogg".to_owned(),
            byte_len: 500,
            sha256: dummy_hash(40),
            duration: DurationInput::Milliseconds(5000),
            start: MemberStartEvidence {
                instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        }],
        NY,
        default_settings(),
    )
    .unwrap();

    let id = plan.members[0].identity.clone();
    insert_tile_claims(
        journal,
        "20260114_100005",
        std::slice::from_ref(&id),
        DEFAULT_WRITE_BUSY_TIMEOUT,
    )
    .unwrap();

    let db_path = claim_db_path(journal);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute_batch("BEGIN EXCLUSIVE;").unwrap();

    let res = find_held_members(journal, None, &plan.members, Duration::ZERO);
    assert!(matches!(res, Err(AudioArchiveError::ClaimDatabaseBusy(_))));

    conn.execute_batch("ROLLBACK;").unwrap();
}

#[cfg(feature = "full-tests")]
#[test]
fn claim_row_with_dot_import_id_returns_claim_database_corrupt() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();

    let plan = plan_audio_archive(
        &[AudioArchiveMemberInput {
            original_relative_name: "dot_id.ogg".to_owned(),
            byte_len: 500,
            sha256: dummy_hash(41),
            duration: DurationInput::Milliseconds(5000),
            start: MemberStartEvidence {
                instant: Some("2026-01-14T21:00:00.000Z".to_owned()),
                local_civil: None,
                source_zone: None,
                ..Default::default()
            },
            run_note: None,
        }],
        NY,
        default_settings(),
    )
    .unwrap();

    let id = plan.members[0].identity.clone();
    insert_tile_claims(
        journal,
        "20260114_100006",
        std::slice::from_ref(&id),
        DEFAULT_WRITE_BUSY_TIMEOUT,
    )
    .unwrap();

    let db_path = claim_db_path(journal);
    let conn = rusqlite::Connection::open(&db_path).unwrap();
    conn.execute(
        "UPDATE audio_archive_claim SET import_id = '.' WHERE sha256 = ?;",
        rusqlite::params![id.sha256],
    )
    .unwrap();

    let res = find_held_members(journal, None, &plan.members, DEFAULT_WRITE_BUSY_TIMEOUT);
    assert!(matches!(
        res,
        Err(AudioArchiveError::ClaimDatabaseCorrupt(_))
    ));

    let count: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM audio_archive_claim WHERE import_id = '.';",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 1);
}

#[cfg(feature = "full-tests")]
#[test]
fn unsafe_or_missing_saved_raws_are_unavailable_without_blocking_other_work() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let sources = [
        storage_member(90, "2026-01-14T21:00:00Z", 1000),
        storage_member(91, "2026-01-14T21:05:00Z", 1000),
        storage_member(92, "2026-01-14T21:10:00Z", 1000),
    ];
    let materials: HashMap<_, _> = sources
        .iter()
        .map(|(member, bytes)| (member.sha256.clone(), bytes.clone()))
        .collect();
    let members: Vec<_> = sources.iter().map(|(member, _)| member.clone()).collect();
    let result = storage_publish(journal, None, &members, &materials, None).unwrap();
    for (index, part) in result.record.parts.iter().take(2).enumerate() {
        let raw = journal
            .join("chronicle")
            .join(&part.day)
            .join(&result.record.settings.stream)
            .join(&part.segment_key)
            .join(&part.stored_name);
        fs::remove_file(&raw).unwrap();
        if index == 0 {
            fs::create_dir(&raw).unwrap();
        }
        #[cfg(unix)]
        if index == 1 {
            std::os::unix::fs::symlink("unavailable-target", &raw).unwrap();
        }
    }
    let batch = update_pending_archive_tiles(journal, 64).unwrap();
    assert!(batch.errors.is_empty());
    assert_eq!(batch.tiles.len(), 1);
    assert_eq!(batch.tiles[0].segment_key, "161000_300");
    let status = ArchiveStore::open(journal, &result.import_id, false)
        .unwrap()
        .status()
        .unwrap();
    assert_eq!(
        (
            status.unavailable_parts,
            status.processed_parts,
            status.pending_processing
        ),
        (2, 0, 1)
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn whole_saved_tile_absence_never_recreates_and_whole_run_absence_releases_identity() {
    for fault in [None, Some(ArchiveFault::AfterPublishBeforeSavedMark)] {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();
        let (member, bytes) = storage_member(101, "2026-01-14T21:00:00Z", 7500);
        let materials = HashMap::from([(member.sha256.clone(), bytes)]);
        let initial = storage_publish(
            journal,
            None,
            std::slice::from_ref(&member),
            &materials,
            fault,
        );
        if fault.is_none() {
            assert!(initial.is_ok());
        } else {
            assert!(matches!(
                initial,
                Err(AudioArchiveError::Fault(
                    ArchiveFault::AfterPublishBeforeSavedMark
                ))
            ));
        }
        let id = storage_run(journal);
        let record = read_audio_archive(journal, &id).unwrap().unwrap();
        let part = &record.parts[0];
        let tile = journal
            .join("chronicle")
            .join(&part.day)
            .join(&record.settings.stream)
            .join(&part.segment_key);
        assert!(tile.join(&part.stored_name).is_file());
        fs::remove_dir_all(&tile).unwrap();
        let resumed = storage_publish(
            journal,
            Some(&id),
            std::slice::from_ref(&member),
            &materials,
            None,
        )
        .unwrap();
        assert!(!tile.exists());
        assert_eq!(
            resumed.record.parts[0].outcome,
            PartOutcome::SavedButUnavailable
        );
        let status = ArchiveStore::open(journal, &id, false)
            .unwrap()
            .status()
            .unwrap();
        assert_eq!(
            (
                status.saved_parts,
                status.unavailable_parts,
                status.processed_parts,
                status.pending_processing
            ),
            (1, 1, 0, 0)
        );
        let next = publish_audio_archive(
            journal,
            AudioArchivePublishOptions {
                import_id: None,
                members: vec![member.clone()],
                materializer: &materials,
                owner_zone: NY,
                settings: AudioArchiveSettings {
                    qualifier: "second-archive".to_owned(),
                    archive_identity: "second".to_owned(),
                },
                source_timestamp: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            next.record.members[0].disposition,
            MemberDisposition::LeftOut(LeftOutReason::HeldByExistingRun)
        );
        assert!(next.record.parts.is_empty());
        fs::remove_dir_all(journal.join("imports").join(&id)).unwrap();
        let released = publish_audio_archive(
            journal,
            AudioArchivePublishOptions {
                import_id: None,
                members: vec![member],
                materializer: &materials,
                owner_zone: NY,
                settings: AudioArchiveSettings {
                    qualifier: "released-archive".to_owned(),
                    archive_identity: "released".to_owned(),
                },
                source_timestamp: None,
            },
            None,
        )
        .unwrap();
        assert_eq!(
            released.record.members[0].disposition,
            MemberDisposition::Included
        );
        assert_eq!(released.parts_saved, 1);
    }
}

#[cfg(feature = "full-tests")]
#[test]
fn publication_without_notification_is_recovered_by_discovery_without_rewriting_raw() {
    for fault in [
        ArchiveFault::AfterPublishBeforeSavedMark,
        ArchiveFault::AfterSavedMarkBeforeEmit,
    ] {
        let temp = TempDir::new().unwrap();
        let journal = temp.path();
        let (member, bytes) = storage_member(102, "2026-01-14T21:00:00Z", 7500);
        let materials = HashMap::from([(member.sha256.clone(), bytes.clone())]);
        assert!(matches!(
            storage_publish(journal, None, &[member], &materials, Some(fault)),
            Err(AudioArchiveError::Fault(_))
        ));
        let id = storage_run(journal);
        let record = read_audio_archive(journal, &id).unwrap().unwrap();
        let part = &record.parts[0];
        let raw = journal
            .join("chronicle")
            .join(&part.day)
            .join(&record.settings.stream)
            .join(&part.segment_key)
            .join(&part.stored_name);
        let before = fs::metadata(&raw).unwrap();
        let batch = update_pending_archive_tiles(journal, 64).unwrap();
        assert!(batch.errors.is_empty());
        assert_eq!(batch.tiles.len(), 1);
        assert_eq!(fs::read(&raw).unwrap(), bytes);
        let after = fs::metadata(&raw).unwrap();
        assert_eq!(before.modified().unwrap(), after.modified().unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            assert_eq!(before.ino(), after.ino());
        }
        let status = ArchiveStore::open(journal, &id, false)
            .unwrap()
            .status()
            .unwrap();
        assert_eq!((status.saved_parts, status.pending_processing), (1, 1));
        finish_archive_part(
            journal,
            &id,
            &part.day,
            &part.segment_key,
            &part.stored_name,
            false,
        )
        .unwrap();
        assert!(
            update_pending_archive_tiles(journal, 64)
                .unwrap()
                .tiles
                .is_empty()
        );
    }
}

#[cfg(feature = "full-tests")]
#[test]
fn archive_progress_precedes_chronicle_and_only_counts_durable_saves() {
    use std::sync::{Arc, Mutex};
    for fault in [
        Some(ArchiveFault::AfterRecordFirstSaved),
        Some(ArchiveFault::AfterTileBytesStaged),
        Some(ArchiveFault::AfterSavedMarkBeforeEmit),
        None,
    ] {
        let root = TempDir::new().unwrap();
        let (member, bytes) = storage_member(991, "2026-01-14T21:00:00Z", 7500);
        let materials = HashMap::from([(member.sha256.clone(), bytes)]);
        let events = Arc::new(Mutex::new(Vec::new()));
        let captured = events.clone();
        let journal = root.path().to_path_buf();
        let result = publish_audio_archive(
            root.path(),
            AudioArchivePublishOptions {
                import_id: None,
                members: vec![member],
                materializer: &materials,
                owner_zone: NY,
                settings: default_settings(),
                source_timestamp: None,
            },
            Some(AudioArchivePublishSeams {
                fault,
                heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
                emit_observed: None,
                progress: Some(Arc::new(move |event| {
                    if event.phase == ArchiveProgressPhase::Admitted {
                        assert!(!journal.join("chronicle").exists());
                        assert_eq!(event.status.size.members, 1);
                        assert_eq!(event.status.saved_parts, 0);
                    } else {
                        assert_eq!(event.status.saved_parts, 1);
                        assert!(journal.join("chronicle").is_dir());
                    }
                    captured.lock().unwrap().push(event.phase);
                    Ok(())
                })),
            }),
        );
        assert_eq!(result.is_ok(), fault.is_none());
        let events = events.lock().unwrap();
        assert_eq!(events[0], ArchiveProgressPhase::Admitted);
        let saved = fault.is_none() || fault == Some(ArchiveFault::AfterSavedMarkBeforeEmit);
        assert_eq!(events.len(), if saved { 2 } else { 1 });
        assert!(
            archive_day_has_pending(root.path(), "20260114").unwrap()
                == (fault != Some(ArchiveFault::AfterRecordFirstSaved)
                    && fault != Some(ArchiveFault::AfterTileBytesStaged))
        );
    }
}

#[cfg(feature = "full-tests")]
struct StopAfterWrite {
    bytes: HashMap<String, Vec<u8>>,
    /// Request stop after this 1-based materialize call. Zero never requests it.
    stop_on_call: usize,
    calls: std::cell::Cell<usize>,
    stop: crate::WorkStop,
}

#[cfg(feature = "full-tests")]
impl AudioArchiveMaterializer for StopAfterWrite {
    fn materialize(
        &self,
        member: &AudioArchiveRecordMember,
        part: &AudioArchiveRecordPart,
        target: &std::path::Path,
    ) -> Result<(), AudioArchiveError> {
        self.bytes.materialize(member, part, target)?;
        let call = self.calls.get() + 1;
        self.calls.set(call);
        if self.stop_on_call == call {
            self.stop.request();
        }
        Ok(())
    }
}

#[cfg(feature = "full-tests")]
fn publish_with_materializer(
    journal: &std::path::Path,
    id: Option<&str>,
    members: Vec<AudioArchiveMemberInput>,
    materializer: &dyn AudioArchiveMaterializer,
    fault: Option<ArchiveFault>,
) -> Result<AudioArchivePublishResult, AudioArchiveError> {
    publish_audio_archive(
        journal,
        AudioArchivePublishOptions {
            import_id: id,
            members,
            materializer,
            owner_zone: NY,
            settings: default_settings(),
            source_timestamp: None,
        },
        Some(AudioArchivePublishSeams {
            fault,
            emit_observed: None,
            heartbeat_spawner: Some(|_, _| Ok(std::thread::spawn(|| {}))),
            progress: None,
        }),
    )
}

#[cfg(feature = "full-tests")]
fn claim_rows(journal: &std::path::Path) -> i64 {
    let path = claim_db_path(journal);
    if !path.is_file() {
        return 0;
    }
    use rusqlite::OptionalExtension;
    let connection = rusqlite::Connection::open(path).unwrap();
    let present: Option<i64> = connection
        .query_row(
            "SELECT 1 FROM sqlite_master WHERE type='table' AND name='audio_archive_claim'",
            [],
            |row| row.get(0),
        )
        .optional()
        .unwrap();
    if present.is_none() {
        return 0;
    }
    connection
        .query_row("SELECT count(*) FROM audio_archive_claim", [], |row| {
            row.get(0)
        })
        .unwrap()
}

#[cfg(feature = "full-tests")]
fn files_under(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    if !root.exists() {
        return found;
    }
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                pending.push(path);
            } else {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

#[cfg(feature = "full-tests")]
fn two_tile_members() -> (Vec<AudioArchiveMemberInput>, HashMap<String, Vec<u8>>) {
    let (first, first_bytes) = storage_member(1, "2026-01-14T21:00:00Z", 5_000);
    let (second, second_bytes) = storage_member(2, "2026-01-14T21:10:00Z", 5_000);
    let bytes = HashMap::from([
        (first.sha256.clone(), first_bytes),
        (second.sha256.clone(), second_bytes),
    ]);
    (vec![first, second], bytes)
}

#[cfg(feature = "full-tests")]
fn part_geometry(record: &AudioArchiveRecord) -> Vec<(String, i64, i64, i64)> {
    record
        .parts
        .iter()
        .map(|part| {
            (
                part.stored_name.clone(),
                part.offset_ms,
                part.origin_ms,
                part.length_ms,
            )
        })
        .collect()
}

#[cfg(feature = "full-tests")]
#[test]
fn stop_during_the_only_part_publishes_nothing() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (member, bytes) = storage_member(11, "2026-01-14T21:00:00Z", 5_000);
    let materials = HashMap::from([(member.sha256.clone(), bytes.clone())]);
    let stop = crate::WorkStop::new();
    let _guard = crate::WorkStopGuard::install(&stop);
    let materializer = StopAfterWrite {
        bytes: materials,
        stop_on_call: 1,
        calls: std::cell::Cell::new(0),
        stop: stop.clone(),
    };
    let error = publish_with_materializer(journal, None, vec![member], &materializer, None)
        .expect_err("a stop during staging publishes nothing");
    assert_eq!(error.to_string(), "import stopped");
    assert_eq!(materializer.calls.get(), 1);
    assert_eq!(claim_rows(journal), 0);
    assert!(files_under(&journal.join("chronicle")).is_empty());
    let staged: Vec<_> = files_under(&journal.join("imports"))
        .into_iter()
        .filter(|path| path.components().any(|part| part.as_os_str() == "staged"))
        .collect();
    assert_eq!(staged.len(), 1);
    assert_eq!(fs::read(&staged[0]).unwrap(), bytes);
    assert!(files_under(&journal.join("imports")).iter().all(|path| {
        !path
            .file_name()
            .is_some_and(|name| name.to_string_lossy().ends_with(".intent.json"))
    }));
}

#[cfg(feature = "full-tests")]
#[test]
fn stop_before_publication_does_not_materialize() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (members, bytes) = two_tile_members();
    let stop = crate::WorkStop::new();
    stop.request();
    let _guard = crate::WorkStopGuard::install(&stop);
    let materializer = StopAfterWrite {
        bytes,
        stop_on_call: 0,
        calls: std::cell::Cell::new(0),
        stop: stop.clone(),
    };
    let error = publish_with_materializer(journal, None, members, &materializer, None)
        .expect_err("a stop before the first tile publishes nothing");
    assert_eq!(error.to_string(), "import stopped");
    assert_eq!(materializer.calls.get(), 0);
    assert_eq!(claim_rows(journal), 0);
    assert!(files_under(&journal.join("chronicle")).is_empty());
    assert!(
        files_under(&journal.join("imports"))
            .iter()
            .all(|path| !path.components().any(|part| part.as_os_str() == "staged"))
    );
}

#[cfg(feature = "full-tests")]
#[test]
fn stop_during_the_second_tile_keeps_the_first_and_does_not_claim_the_second() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (members, bytes) = two_tile_members();
    let stop = crate::WorkStop::new();
    let _guard = crate::WorkStopGuard::install(&stop);
    let materializer = StopAfterWrite {
        bytes: bytes.clone(),
        stop_on_call: 2,
        calls: std::cell::Cell::new(0),
        stop: stop.clone(),
    };
    let error = publish_with_materializer(journal, None, members, &materializer, None)
        .expect_err("the second tile stops before its claim");
    assert_eq!(error.to_string(), "import stopped");
    assert_eq!(materializer.calls.get(), 2);
    assert_eq!(claim_rows(journal), 1);
    let id = storage_run(journal);
    let record = read_audio_archive(journal, &id).unwrap().unwrap();
    let saved: Vec<_> = record
        .parts
        .iter()
        .filter(|part| part.outcome == PartOutcome::Saved)
        .collect();
    let waiting: Vec<_> = record
        .parts
        .iter()
        .filter(|part| part.outcome == PartOutcome::Planned)
        .collect();
    assert_eq!(saved.len(), 1);
    assert_eq!(waiting.len(), 1);
    let chronicle = files_under(&journal.join("chronicle"));
    let raws: Vec<_> = chronicle
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == saved[0].stored_name.as_str())
        })
        .collect();
    assert_eq!(raws.len(), 1);
    assert_eq!(fs::read(raws[0]).unwrap(), bytes[&saved[0].member_sha256]);
    assert!(chronicle.iter().all(|path| {
        path.file_name()
            .is_some_and(|name| name != waiting[0].stored_name.as_str())
    }));
}

#[cfg(feature = "full-tests")]
#[test]
fn a_won_tile_finishes_after_stop_and_the_next_tile_does_not_start() {
    let temp = TempDir::new().unwrap();
    let journal = temp.path();
    let (members, bytes) = two_tile_members();
    let interrupted = publish_with_materializer(
        journal,
        None,
        members.clone(),
        &bytes,
        Some(ArchiveFault::AfterClaimAndIntent),
    );
    assert!(matches!(
        interrupted,
        Err(AudioArchiveError::Fault(ArchiveFault::AfterClaimAndIntent))
    ));
    assert_eq!(claim_rows(journal), 1);
    assert!(files_under(&journal.join("chronicle")).is_empty());
    let id = storage_run(journal);
    let before = read_audio_archive(journal, &id).unwrap().unwrap();
    let geometry = part_geometry(&before);
    let won = before
        .parts
        .iter()
        .find(|part| part.outcome == PartOutcome::Staged)
        .expect("the claimed tile is staged");
    let waiting = before
        .parts
        .iter()
        .find(|part| part.outcome == PartOutcome::Planned)
        .expect("the later tile has not started");
    let won_name = won.stored_name.clone();
    let won_hash = won.member_sha256.clone();
    let waiting_name = waiting.stored_name.clone();
    let intents_before = files_under(&journal.join("imports"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".intent.json"))
        })
        .count();

    let stop = crate::WorkStop::new();
    stop.request();
    let _guard = crate::WorkStopGuard::install(&stop);
    let materializer = StopAfterWrite {
        bytes: bytes.clone(),
        stop_on_call: 0,
        calls: std::cell::Cell::new(0),
        stop: stop.clone(),
    };
    let error = publish_with_materializer(journal, Some(&id), members, &materializer, None)
        .expect_err("stop retains the won tile and refuses the next");
    assert_eq!(error.to_string(), "import stopped");
    assert_eq!(materializer.calls.get(), 0);
    assert_eq!(claim_rows(journal), 1);
    let after = read_audio_archive(journal, &id).unwrap().unwrap();
    assert_eq!(part_geometry(&after), geometry);
    assert_eq!(
        after
            .parts
            .iter()
            .find(|part| part.stored_name == won_name)
            .unwrap()
            .outcome,
        PartOutcome::Saved
    );
    assert_eq!(
        after
            .parts
            .iter()
            .find(|part| part.stored_name == waiting_name)
            .unwrap()
            .outcome,
        PartOutcome::Planned
    );
    let chronicle = files_under(&journal.join("chronicle"));
    let raws: Vec<_> = chronicle
        .iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name == won_name.as_str())
        })
        .collect();
    assert_eq!(raws.len(), 1);
    assert_eq!(fs::read(raws[0]).unwrap(), bytes[&won_hash]);
    assert!(chronicle.iter().all(|path| {
        path.file_name()
            .is_some_and(|name| name != waiting_name.as_str())
    }));
    let intents_after = files_under(&journal.join("imports"))
        .into_iter()
        .filter(|path| {
            path.file_name()
                .is_some_and(|name| name.to_string_lossy().ends_with(".intent.json"))
        })
        .count();
    assert_eq!(intents_after, intents_before);
}
