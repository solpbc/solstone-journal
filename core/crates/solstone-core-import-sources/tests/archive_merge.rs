// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};

use serde_json::{Map, Value, json};
use solstone_core_entity::{
    load_all_journal_entities, read_journal_principal, save_entity_identity,
};
use solstone_core_facets::{add_observation, save_facet_entity_link};
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, EntityDispositionKind, PrincipalAdoption, RetryDisposition,
    SegmentDispositionKind, merge_journal_archive, preview_size,
};
use solstone_core_import_sources::{ArchiveSafetyPhase, ImportSourcesError};
use solstone_core_journal_io::{
    HealthMarkerKind, HealthMarkerState, LockOptions, hold_lock, read_health_marker,
};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipWriter};

static NEXT: AtomicUsize = AtomicUsize::new(0);

#[test]
fn creates_principal_claiming_entity_without_adopting_principal() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &source,
        "other",
        &json!({"id":"other","name":"Other Person","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);
    let options = options(&tree);
    let result = merge_journal_archive(&archive, &target, &options, None).unwrap();
    assert_eq!(
        result.entity_dispositions[0].disposition,
        EntityDispositionKind::Created
    );
    assert_eq!(
        result.entity_dispositions[0].principal_adoption,
        PrincipalAdoption::ClearedOnCreate
    );
    assert_ne!(
        load_all_journal_entities(&target).unwrap()[0]
            .value
            .get("is_principal"),
        Some(&json!(true))
    );
}

#[test]
fn a_source_principal_flag_never_reaches_the_target_in_any_form() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (id, identity) in [
        (
            "office",
            json!({"id":"office","name":"Office","type":"Tool","is_principal":true}),
        ),
        (
            "ada",
            json!({"id":"ada","name":"Ada","type":"Person","is_principal":"yes"}),
        ),
    ] {
        let directory = source.join("entities").join(id);
        fs::create_dir_all(&directory).unwrap();
        fs::write(directory.join("entity.json"), identity.to_string()).unwrap();
    }
    let archive = archive_from(&source, &tree.path);
    let options = options(&tree);
    merge_journal_archive(&archive, &target, &options, None).unwrap();
    for id in ["office", "ada"] {
        let written: Value = serde_json::from_slice(
            &fs::read(target.join("entities").join(id).join("entity.json")).unwrap(),
        )
        .unwrap();
        assert!(written.get("is_principal").is_none(), "{id}: {written}");
    }
    assert_eq!(read_journal_principal(&target).unwrap(), None);
}

#[test]
fn identical_retry_is_noop_and_different_segment_lands_on_a_new_key() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(source.join("chronicle/20260811/120000_60")).unwrap();
    fs::write(source.join("chronicle/20260811/120000_60/value"), b"one").unwrap();
    let archive = archive_from(&source, &tree.path);
    let options = options(&tree);
    let archive_before = fs::read(&archive).unwrap();
    let archive_mtime = fs::metadata(&archive).unwrap().modified().unwrap();
    merge_journal_archive(&archive, &target, &options, None).unwrap();
    let before = fs::read(target.join("chronicle/20260811/120000_60/value")).unwrap();
    let second = merge_journal_archive(&archive, &target, &options, None).unwrap();
    assert_eq!(second.retry_disposition, RetryDisposition::IdempotentNoop);
    assert_eq!(second.entries_written, 0);
    assert_eq!(
        fs::read(target.join("chronicle/20260811/120000_60/value")).unwrap(),
        before
    );

    // Different material under a key this journal already holds lands on the
    // next key, as an upload would, and the merge completes.
    fs::write(source.join("chronicle/20260811/120000_60/value"), b"two").unwrap();
    let collision_archive = archive_from(&source, &tree.path);
    let collision = merge_journal_archive(&collision_archive, &target, &options, None).unwrap();
    assert_eq!(collision.retry_disposition, RetryDisposition::Applied);
    assert_eq!(collision.merge_summary.segments_copied, 1);
    assert_eq!(collision.merge_summary.segments_skipped, 0);
    assert!(collision.segment_dispositions.iter().any(|item| {
        item.day == "20260811"
            && item.stream == "_default"
            && item.key == "120000_60"
            && item.disposition
                == SegmentDispositionKind::CopiedToNewKey {
                    landed_segment: "120000_61".to_owned(),
                }
    }));
    assert_eq!(
        fs::read(target.join("chronicle/20260811/120000_60/value")).unwrap(),
        before
    );
    assert_eq!(
        fs::read(target.join("chronicle/20260811/120000_61/value")).unwrap(),
        b"two"
    );
    assert!(matches!(
        read_health_marker(&target, "20260811", HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Versioned { marker, .. } if marker.generation == 2
    ));

    // Retrying that merge finds the material where it landed.
    let retry = merge_journal_archive(&collision_archive, &target, &options, None).unwrap();
    assert_eq!(retry.retry_disposition, RetryDisposition::IdempotentNoop);
    assert!(
        retry
            .segment_dispositions
            .iter()
            .all(|item| item.key != "120000_60"
                || item.disposition == SegmentDispositionKind::IdenticalExisting)
    );
    assert!(!target.join("chronicle/20260811/120000_62").exists());

    assert_eq!(fs::read(&archive).unwrap(), archive_before);
    assert_eq!(
        fs::metadata(&archive).unwrap().modified().unwrap(),
        archive_mtime
    );
}

#[test]
fn a_segment_the_owner_removed_comes_back_and_a_removal_in_progress_is_skipped() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (segment, value) in [
        ("120000_60", "one"),
        ("130000_60", "two"),
        ("140000_60", "three"),
        ("150000_60", "four"),
    ] {
        fs::create_dir_all(source.join("chronicle/20260811").join(segment)).unwrap();
        fs::write(
            source
                .join("chronicle/20260811")
                .join(segment)
                .join("value"),
            value,
        )
        .unwrap();
    }
    // 160000_60 collides with live content at its own key; the next key ingest
    // would choose is one the owner removed.
    fs::create_dir_all(source.join("chronicle/20260811/160000_60")).unwrap();
    fs::write(source.join("chronicle/20260811/160000_60/value"), b"moved").unwrap();
    let archive = archive_from(&source, &tree.path);
    let options = options(&tree);

    // The owner removed one segment (only its tombstone holds its key), another
    // is mid-removal (its staged removal sits beside where it was), and a third
    // holds a tombstone beside other files (also part-way through).
    fs::create_dir_all(target.join("chronicle/20260811/120000_60")).unwrap();
    fs::write(
        target.join("chronicle/20260811/120000_60/tombstone.json"),
        b"{}",
    )
    .unwrap();
    fs::create_dir_all(target.join("chronicle/20260811/.removing_130000_60")).unwrap();
    fs::create_dir_all(target.join("chronicle/20260811/150000_60")).unwrap();
    fs::write(
        target.join("chronicle/20260811/150000_60/tombstone.json"),
        b"{}",
    )
    .unwrap();
    fs::write(target.join("chronicle/20260811/150000_60/audio.flac"), b"x").unwrap();
    fs::create_dir_all(target.join("chronicle/20260811/160000_60")).unwrap();
    fs::write(
        target.join("chronicle/20260811/160000_60/value"),
        b"already here",
    )
    .unwrap();
    fs::create_dir_all(target.join("chronicle/20260811/160000_61")).unwrap();
    fs::write(
        target.join("chronicle/20260811/160000_61/tombstone.json"),
        b"{}",
    )
    .unwrap();

    let preview = preview_size(&archive, &target).unwrap();
    assert!(
        target
            .join("chronicle/20260811/120000_60/tombstone.json")
            .exists(),
        "preview writes nothing to the journal"
    );
    assert_eq!(preview.recordings.len(), 3);

    let result = merge_journal_archive(&archive, &target, &options, None).unwrap();
    let disposition = |key: &str| {
        result
            .segment_dispositions
            .iter()
            .find(|item| item.key == key)
            .unwrap()
            .disposition
            .clone()
    };
    assert_eq!(disposition("120000_60"), SegmentDispositionKind::Copied);
    assert_eq!(
        disposition("130000_60"),
        SegmentDispositionKind::DeletedByOwner
    );
    assert_eq!(disposition("140000_60"), SegmentDispositionKind::Copied);
    assert_eq!(
        disposition("150000_60"),
        SegmentDispositionKind::DeletedByOwner
    );
    assert_eq!(
        disposition("160000_60"),
        SegmentDispositionKind::CopiedToNewKey {
            landed_segment: "160000_61".to_owned()
        }
    );
    let day = target.join("chronicle/20260811");
    assert_eq!(fs::read(day.join("120000_60/value")).unwrap(), b"one");
    assert!(!day.join("120000_60/tombstone.json").exists());
    assert!(!day.join("120000_61").exists());
    assert!(!day.join("130000_60").exists());
    assert!(day.join(".removing_130000_60").is_dir());
    assert_eq!(fs::read(day.join("140000_60/value")).unwrap(), b"three");
    assert!(day.join("150000_60/tombstone.json").exists());
    assert!(!day.join("150000_60/value").exists());
    assert_eq!(
        fs::read(day.join("160000_60/value")).unwrap(),
        b"already here"
    );
    assert_eq!(fs::read(day.join("160000_61/value")).unwrap(), b"moved");
    assert!(!day.join("160000_61/tombstone.json").exists());
}

#[test]
fn a_moved_segment_never_takes_the_key_another_archive_segment_arrives_with() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (segment, value) in [("120000_60", "moved"), ("120000_61", "own key")] {
        let path = source.join("chronicle/20260811/device").join(segment);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("value"), value).unwrap();
    }
    fs::create_dir_all(target.join("chronicle/20260811/device/120000_60")).unwrap();
    fs::write(
        target.join("chronicle/20260811/device/120000_60/value"),
        b"already here",
    )
    .unwrap();

    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    assert_eq!(result.retry_disposition, RetryDisposition::Applied);
    let stream = target.join("chronicle/20260811/device");
    assert_eq!(
        fs::read(stream.join("120000_60/value")).unwrap(),
        b"already here"
    );
    assert_eq!(
        fs::read(stream.join("120000_61/value")).unwrap(),
        b"own key"
    );
    assert_eq!(fs::read(stream.join("120000_62/value")).unwrap(), b"moved");
}

#[test]
fn a_segment_is_skipped_only_when_every_key_ingest_would_try_differs() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(source.join("chronicle/20260811/120000_60")).unwrap();
    fs::write(source.join("chronicle/20260811/120000_60/value"), b"new").unwrap();
    for duration in 60..60 + solstone_core_ingest_resolve::MAX_INGEST_SEGMENT_ATTEMPTS {
        let path = target
            .join("chronicle/20260811")
            .join(format!("120000_{duration}"));
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("value"), b"old").unwrap();
    }

    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    assert_eq!(result.retry_disposition, RetryDisposition::Incomplete);
    assert_eq!(result.merge_summary.segments_skipped, 1);
    assert_eq!(
        result.segment_dispositions[0].disposition,
        SegmentDispositionKind::DifferingContentCollision
    );
    assert!(!target.join("chronicle/20260811/120000_160").exists());
}

#[test]
fn archive_merge_dirties_exactly_the_days_with_published_segments() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (day, segment) in [("20260809", "090000_60"), ("20260810", "100000_60")] {
        let path = source.join("chronicle").join(day).join(segment);
        fs::create_dir_all(&path).unwrap();
        fs::write(path.join("audio.flac"), day.as_bytes()).unwrap();
    }
    fs::create_dir_all(target.join("chronicle/20260811/110000_60")).unwrap();
    fs::write(
        target.join("chronicle/20260811/110000_60/audio.flac"),
        b"untouched",
    )
    .unwrap();

    let archive = archive_from(&source, &tree.path);
    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    for day in ["20260809", "20260810"] {
        assert!(matches!(
            read_health_marker(&target, day, HealthMarkerKind::Stream).unwrap(),
            HealthMarkerState::Versioned { marker, .. } if marker.generation == 1
        ));
    }
    assert!(matches!(
        read_health_marker(&target, "20260811", HealthMarkerKind::Stream).unwrap(),
        HealthMarkerState::Absent
    ));
}

#[test]
fn archive_marker_failure_is_terminal_after_published_content_is_preserved() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    let segment = source.join("chronicle/20260809/090000_60");
    fs::create_dir_all(&segment).unwrap();
    fs::write(segment.join("audio.flac"), b"published").unwrap();
    fs::create_dir_all(target.join("chronicle/20260809/health/stream.updated")).unwrap();

    let archive = archive_from(&source, &tree.path);
    let error = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap_err();

    match error {
        ImportSourcesError::MergePublishFailed { detail, .. } => {
            assert!(detail.contains("stream marker update failed"), "{detail}");
            assert!(
                detail.contains("published content was not rolled back"),
                "{detail}"
            );
        }
        other => panic!("expected MergePublishFailed, got {other:?}"),
    }
    assert_eq!(
        fs::read(target.join("chronicle/20260809/090000_60/audio.flac")).unwrap(),
        b"published"
    );
}

#[test]
fn ambiguous_and_id_colliding_entities_are_staged_for_later_review() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &target,
        "sam-one",
        &json!({"id":"sam-one","name":"Sam Same","type":"Person"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &target,
        "sam-two",
        &json!({"id":"sam-two","name":"Sam Same","type":"Person"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &target,
        "id-collision",
        &json!({"id":"id-collision","name":"Existing","type":"Person"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &source,
        "source-sam",
        &json!({"id":"source-sam","name":"Sam Same","type":"Person"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &source,
        "id-collision",
        &json!({"id":"id-collision","name":"","type":"Person"}),
        None,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert!(
        result
            .entity_dispositions
            .iter()
            .any(|item| item.disposition == EntityDispositionKind::StagedAmbiguous)
    );
    assert!(
        result
            .entity_dispositions
            .iter()
            .any(|item| item.disposition == EntityDispositionKind::StagedIdCollision),
        "{:?}",
        result.entity_dispositions
    );
    for item in result
        .entity_dispositions
        .iter()
        .filter_map(|item| item.staging_path.as_ref())
    {
        assert!(item.is_file());
    }
}

#[test]
fn unrelated_names_are_created_rather_than_set_aside_as_near_matches() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &target,
        "ada-lovelace",
        &json!({"id":"ada-lovelace","name":"Ada Lovelace","type":"Person"}),
        None,
    )
    .unwrap();
    for (id, name) in [
        ("grace-hopper", "Grace Hopper"),
        ("alan-turing", "Alan Turing"),
    ] {
        save_entity_identity(
            &source,
            id,
            &json!({"id":id,"name":name,"type":"Person"}),
            None,
        )
        .unwrap();
    }
    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert!(
        result
            .entity_dispositions
            .iter()
            .all(|item| item.disposition == EntityDispositionKind::Created),
        "{:?}",
        result.entity_dispositions
    );
    assert!(!target.join("entities/ambiguities.jsonl").exists());
}

#[test]
fn validation_refuses_unsafe_entries_and_busy_lock_reports_metadata_remedy() {
    let tree = TempTree::new();
    let unsafe_archive = tree.path.join("unsafe.zip");
    let mut writer = ZipWriter::new(File::create(&unsafe_archive).unwrap());
    writer
        .start_file("../escape", SimpleFileOptions::default())
        .unwrap();
    writer.write_all(b"no").unwrap();
    writer.finish().unwrap();
    let unsafe_error = merge_journal_archive(
        &unsafe_archive,
        &tree.path.join("target"),
        &options(&tree),
        None,
    )
    .unwrap_err();
    assert!(matches!(
        unsafe_error,
        ImportSourcesError::ArchiveUnsafeEntry {
            phase: ArchiveSafetyPhase::Validation,
            ..
        }
    ));
    let source = tree.path.join("source");
    fs::create_dir_all(source.join("chronicle/20260811/120000_60")).unwrap();
    fs::write(source.join("chronicle/20260811/120000_60/item"), b"x").unwrap();
    let archive = archive_from(&source, &tree.path);
    let target = tree.path.join("target");
    let protected = target.join("health/locks/archive-merge");
    let _lock = hold_lock(&protected, LockOptions::default()).unwrap();
    fs::write(
        target.join("health/locks/archive-merge.owner.json"),
        b"not-json",
    )
    .unwrap();
    let mut locked_options = options(&tree);
    locked_options.lock_options.timeout = std::time::Duration::ZERO;
    let lock_error = merge_journal_archive(&archive, &target, &locked_options, None).unwrap_err();
    match lock_error {
        ImportSourcesError::LockBusy {
            owner_metadata_path,
            owner,
            remedy,
            ..
        } => {
            assert_eq!(owner, None);
            assert!(owner_metadata_path.ends_with("archive-merge.owner.json"));
            assert!(remedy.contains("owner metadata"));
        }
        other => panic!("unexpected error: {other:?}"),
    }
}

#[test]
fn extraction_fails_partway_and_leaves_no_extraction_directory() {
    let tree = TempTree::new();
    let archive = tree.path.join("corrupt.zip");
    let mut writer = ZipWriter::new(File::create(&archive).unwrap());
    let stored = SimpleFileOptions::default().compression_method(CompressionMethod::Stored);
    // The first safe entry becomes a regular file. The following safe descendant can pass
    // validation but cannot be extracted below that file, exercising the second layer.
    writer.start_file("chronicle", stored).unwrap();
    writer.write_all(b"not a directory").unwrap();
    writer
        .start_file("chronicle/20260811/120000_60/second", stored)
        .unwrap();
    writer.write_all(b"second").unwrap();
    writer.finish().unwrap();
    let result = merge_journal_archive(&archive, &tree.path.join("target"), &options(&tree), None);
    assert!(
        matches!(result, Err(ImportSourcesError::ExtractionFailed { .. })),
        "{result:?}"
    );
    let work = tree.path.join("work");
    assert!(
        !work.exists()
            || fs::read_dir(work).unwrap().all(|entry| !entry
                .unwrap()
                .file_name()
                .to_string_lossy()
                .starts_with("extract-"))
    );
}

#[test]
fn facet_logs_and_activities_are_idempotent_on_retry() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(source.join("facets/work/logs")).unwrap();
    fs::create_dir_all(source.join("facets/work/activities")).unwrap();
    fs::write(
        source.join("facets/work/logs/events.log"),
        b"first\nsecond\n",
    )
    .unwrap();
    fs::write(
        source.join("facets/work/activities/events.jsonl"),
        b"{\"id\":\"activity-one\"}\n",
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);
    let options = options(&tree);

    merge_journal_archive(&archive, &target, &options, None).unwrap();
    let log_path = target.join("facets/work/logs/events.log");
    let activity_path = target.join("facets/work/activities/events.jsonl");
    let log_before = fs::read(&log_path).unwrap();
    let activities_before = fs::read(&activity_path).unwrap();

    let retry = merge_journal_archive(&archive, &target, &options, None).unwrap();
    assert_eq!(fs::read(&log_path).unwrap(), log_before);
    assert_eq!(fs::read(&activity_path).unwrap(), activities_before);
    assert_eq!(retry.retry_disposition, RetryDisposition::IdempotentNoop);
}

#[test]
fn idless_activity_rows_are_idempotent_on_retry() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(source.join("facets/work/activities")).unwrap();
    fs::write(
        source.join("facets/work/activities/idless.jsonl"),
        b"{\"summary\":\"no id\"}\n",
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);
    let options = options(&tree);

    merge_journal_archive(&archive, &target, &options, None).unwrap();
    let activity_path = target.join("facets/work/activities/idless.jsonl");
    let before = fs::read(&activity_path).unwrap();
    let retry = merge_journal_archive(&archive, &target, &options, None).unwrap();

    assert_eq!(fs::read(&activity_path).unwrap(), before);
    assert_eq!(retry.retry_disposition, RetryDisposition::IdempotentNoop);
}

#[test]
fn existing_facet_merges_entity_links_and_observations() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    let target_fields = Map::from_iter([("role".to_owned(), Value::String("owner".to_owned()))]);
    save_facet_entity_link(&target, "work", "existing", "existing", &target_fields).unwrap();

    let source_fields = Map::from_iter([("role".to_owned(), Value::String("member".to_owned()))]);
    save_facet_entity_link(
        &source,
        "work",
        "from-archive",
        "from-archive",
        &source_fields,
    )
    .unwrap();
    add_observation(
        &source,
        "work",
        "from-archive",
        "from archive",
        Some("2026-08-11"),
        None,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);

    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    let link: Value = serde_json::from_slice(
        &fs::read(target.join("facets/work/entities/from-archive/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(link["entity_id"], "from-archive");
    assert_eq!(link["role"], "member");
    let observations =
        fs::read_to_string(target.join("facets/work/entities/from-archive/observations.jsonl"))
            .unwrap();
    assert!(observations.contains("from archive"));
}

#[test]
fn staged_entity_id_cannot_escape_the_staging_root() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for id in ["first", "second"] {
        save_entity_identity(
            &target,
            id,
            &json!({"id":id,"name":"Ambiguous Name","type":"Person"}),
            None,
        )
        .unwrap();
    }
    fs::create_dir_all(source.join("entities/safe")).unwrap();
    fs::write(
        source.join("entities/safe/entity.json"),
        br#"{"id":"../../../../escape","name":"Ambiguous Name","type":"Person"}"#,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);

    let error = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap_err();
    assert!(matches!(error, ImportSourcesError::StagingWrite { .. }));
    assert!(!tree.path.join("escape.json").exists());
}

#[test]
fn new_principal_claim_conflict_is_reported_without_adopting_the_claim() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &target,
        "owner",
        &json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &source,
        "new-person",
        &json!({"id":"new-person","name":"Qxjvplmzt","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    assert!(read_journal_principal(&target).unwrap().is_some());
    assert!(read_journal_principal(&source).unwrap().is_some());
    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    let disposition = result
        .entity_dispositions
        .iter()
        .find(|item| item.source_id == "new-person")
        .unwrap();
    assert_eq!(
        disposition.principal_adoption,
        PrincipalAdoption::ConflictReportedSeparately
    );
    assert_eq!(
        load_all_journal_entities(&target)
            .unwrap()
            .into_iter()
            .find(|entity| entity.id == "new-person")
            .unwrap()
            .value
            .get("is_principal"),
        None
    );
    let collision = result.principal_collision.unwrap();
    assert_eq!(collision.target_entity_id, "owner");
    assert_eq!(collision.source_entity_id, "new-person");
    assert_eq!(collision.target_name, "Owner");
    assert_eq!(collision.source_name, "Qxjvplmzt");
}

#[test]
fn staged_principal_claim_reports_collision_marked_staged() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (id, value) in [
        (
            "owner",
            json!({"id":"owner","name":"Owner","type":"Person","is_principal":true}),
        ),
        (
            "sam-one",
            json!({"id":"sam-one","name":"Sam Same","type":"Person"}),
        ),
        (
            "sam-two",
            json!({"id":"sam-two","name":"Sam Same","type":"Person"}),
        ),
    ] {
        save_entity_identity(&target, id, &value, None).unwrap();
    }
    save_entity_identity(
        &source,
        "source-sam",
        &json!({"id":"source-sam","name":"Sam Same","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    let disposition = result
        .entity_dispositions
        .iter()
        .find(|item| item.source_id == "source-sam")
        .unwrap();
    assert_eq!(
        disposition.disposition,
        EntityDispositionKind::StagedAmbiguous
    );
    assert_eq!(
        disposition.principal_adoption,
        PrincipalAdoption::ConflictReportedSeparately
    );
    let collision = result.principal_collision.unwrap();
    assert_eq!(collision.target_entity_id, "owner");
    assert_eq!(collision.source_entity_id, "source-sam");
    assert_eq!(collision.source_name, "Sam Same");
    assert!(collision.staged);
}

#[test]
fn same_name_principal_claim_reports_collision() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &target,
        "owner",
        &json!({"id":"owner","name":"Shared Name","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &source,
        "source-principal",
        &json!({"id":"source-principal","name":"Shared Name","type":"Person","is_principal":true}),
        None,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    let collision = result.principal_collision.unwrap();
    assert_eq!(collision.target_entity_id, "owner");
    assert_eq!(collision.source_entity_id, "source-principal");
    assert_eq!(collision.target_name, "Shared Name");
    assert_eq!(collision.source_name, "Shared Name");
}

#[test]
fn facet_failure_after_staged_segments_leaves_target_unchanged() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(target.join("chronicle/20260101/120000_60")).unwrap();
    fs::write(
        target.join("chronicle/20260101/120000_60/keep"),
        b"keep-day",
    )
    .unwrap();
    save_entity_identity(
        &target,
        "keep",
        &json!({"id":"keep","name":"Keep","type":"Person"}),
        None,
    )
    .unwrap();
    fs::create_dir_all(target.join("facets/keep")).unwrap();
    fs::write(
        target.join("facets/keep/facet.json"),
        b"{\"name\":\"keep\"}",
    )
    .unwrap();
    fs::create_dir_all(target.join("imports/keep")).unwrap();
    fs::write(target.join("imports/keep/file"), b"keep-import").unwrap();
    fs::create_dir_all(source.join("chronicle/20260311/120000_60")).unwrap();
    fs::write(
        source.join("chronicle/20260311/120000_60/value"),
        b"new-day",
    )
    .unwrap();
    save_entity_identity(
        &source,
        "fresh",
        &json!({"id":"fresh","name":"Fresh","type":"Person"}),
        None,
    )
    .unwrap();
    fs::create_dir_all(source.join("facets/keep/entities/bad")).unwrap();
    fs::write(
        source.join("facets/keep/entities/bad/entity.json"),
        b"not-json",
    )
    .unwrap();
    fs::create_dir_all(source.join("imports/new-id")).unwrap();
    fs::write(source.join("imports/new-id/file"), b"new-import").unwrap();
    let before = family_snapshot(&target);
    let archive = archive_from(&source, &tree.path);

    let error = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap_err();
    assert!(
        matches!(error, ImportSourcesError::FacetMerge { .. }),
        "{error:?}"
    );
    assert!(!matches!(
        error,
        ImportSourcesError::MergePublishFailed { .. }
    ));
    let mut after = family_snapshot(&target);
    // A segment claim keeps its stable lock entry even when publication rolls back.
    assert_eq!(
        after.remove("chronicle/20260311/120000_60.lock"),
        Some(Vec::new())
    );
    assert_eq!(after, before);
    let claimed_day = target.join("chronicle/20260311");
    let entries = fs::read_dir(&claimed_day)
        .unwrap()
        .map(|entry| entry.unwrap())
        .collect::<Vec<_>>();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].file_name(), "120000_60.lock");
    assert!(entries[0].file_type().unwrap().is_file());
    assert!(!claimed_day.join("120000_60").exists());
    assert!(find_named(&tree.path.join("work"), "publish-undo").is_none());
}

#[test]
fn extra_zip_members_are_skipped_and_absent_from_target() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(source.join("chronicle/20260311/120000_60")).unwrap();
    fs::write(source.join("chronicle/20260311/120000_60/value"), b"day").unwrap();
    fs::create_dir_all(source.join("health/pruning-runs")).unwrap();
    fs::write(source.join("health/pruning-runs/x.jsonl"), b"{}\n").unwrap();
    fs::write(source.join("_export.json"), b"{}").unwrap();
    fs::create_dir_all(source.join("identity")).unwrap();
    fs::write(source.join("identity/owner.md"), b"# owner").unwrap();
    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert!(target.join("chronicle/20260311/120000_60/value").is_file());
    assert!(!target.join("health/pruning-runs").exists());
    assert!(!target.join("_export.json").exists());
    assert!(!target.join("identity").exists());
    let log = fs::read_to_string(&result.decision_log_path).unwrap();
    assert!(log.contains("\"state\":\"skipped\""));
    assert!(log.contains("_export.json"));
    assert!(log.contains("identity"));
    assert!(log.contains("health"));
}

fn family_snapshot(root: &Path) -> std::collections::BTreeMap<String, Vec<u8>> {
    let mut files = std::collections::BTreeMap::new();
    for family in ["chronicle", "entities", "facets", "imports"] {
        let path = root.join(family);
        if !path.exists() {
            continue;
        }
        fn visit(
            root: &Path,
            current: &Path,
            files: &mut std::collections::BTreeMap<String, Vec<u8>>,
        ) {
            let Ok(entries) = fs::read_dir(current) else {
                return;
            };
            for entry in entries.filter_map(Result::ok) {
                let ty = entry.file_type().ok();
                if ty.as_ref().is_some_and(std::fs::FileType::is_dir) {
                    visit(root, &entry.path(), files);
                } else if ty.as_ref().is_some_and(std::fs::FileType::is_file) {
                    let relative = entry
                        .path()
                        .strip_prefix(root)
                        .expect("under root")
                        .to_string_lossy()
                        .replace('\\', "/");
                    files.insert(relative, fs::read(entry.path()).unwrap());
                }
            }
        }
        visit(root, &path, &mut files);
    }
    files
}

fn find_named(root: &Path, name: &str) -> Option<PathBuf> {
    fn visit(current: &Path, name: &str) -> Option<PathBuf> {
        let entries = fs::read_dir(current).ok()?;
        for entry in entries.filter_map(Result::ok) {
            if entry.file_name() == name {
                return Some(entry.path());
            }
            if entry.file_type().ok()?.is_dir()
                && let Some(found) = visit(&entry.path(), name)
            {
                return Some(found);
            }
        }
        None
    }
    visit(root, name)
}

fn options(tree: &TempTree) -> ArchiveMergeOptions {
    ArchiveMergeOptions {
        working_root: tree.path.join("work"),
        ..ArchiveMergeOptions::default()
    }
}
fn archive_from(source: &Path, tree: &Path) -> PathBuf {
    let archive = tree.join(format!("{}.zip", NEXT.fetch_add(1, Ordering::Relaxed)));
    let mut writer = ZipWriter::new(File::create(&archive).unwrap());
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    fn add(writer: &mut ZipWriter<File>, root: &Path, current: &Path, options: SimpleFileOptions) {
        let mut entries = fs::read_dir(current)
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        entries.sort_by_key(|entry| entry.file_name());
        for entry in entries {
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                add(writer, root, &path, options);
            } else {
                writer
                    .start_file(
                        path.strip_prefix(root)
                            .unwrap()
                            .to_string_lossy()
                            .replace('\\', "/"),
                        options,
                    )
                    .unwrap();
                writer.write_all(&fs::read(path).unwrap()).unwrap();
            }
        }
    }
    add(&mut writer, source, source, options);
    writer.finish().unwrap();
    archive
}
struct TempTree {
    path: PathBuf,
}
impl TempTree {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "solstone-archive-merge-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self { path }
    }
}
impl Drop for TempTree {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.path).unwrap();
    }
}

fn write(root: &Path, rel: &str, bytes: &[u8]) {
    let path = root.join(rel);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

/// A body window as a real import leaves one: rows, and nothing else.
fn body_window(root: &Path, day: &str, key: &str, row_id: &str) {
    let row = json!({"kind": "reading", "id": row_id, "source": "apple_health",
        "type": "HKQuantityTypeIdentifierHeartRate", "label": "heart rate", "value": "60",
        "at": format!("2026-01-10 09:00:00 -0700")});
    write(
        root,
        &format!("chronicle/{day}/import.apple_health/{key}/body.jsonl"),
        format!("{row}\n").as_bytes(),
    );
}

const APPLE_ZONE: &str = "imports/.apple_health-zone.json";
const DENVER: &[u8] = br#"{"zone":"America/Denver"}"#;
const NEW_YORK: &[u8] = br#"{"zone":"America/New_York"}"#;

#[test]
fn a_merged_body_source_keeps_its_zone_and_leaves_another_journals_run_state_behind() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    body_window(&source, "20260110", "090000_3600", "r1");
    write(&source, APPLE_ZONE, DENVER);
    write(&source, "imports/.apple_health-run/state.json", b"{}");
    write(&source, "imports/.oura-run/state.json", b"{}");
    // Old bundles, native and Python-era, are carried as they always were.
    write(
        &source,
        "imports/body-01ARZ3NDEKTSV4RRFFQ69G5FAV/manifest.json",
        b"{}",
    );
    write(&source, "imports/20240101_120000/manifest.json", b"{}");

    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    assert_eq!(fs::read(target.join(APPLE_ZONE)).unwrap(), DENVER);
    assert!(
        target
            .join("chronicle/20260110/import.apple_health/090000_3600/body.jsonl")
            .is_file()
    );
    assert!(!target.join("imports/.apple_health-run").exists());
    assert!(!target.join("imports/.oura-run").exists());
    assert!(
        target
            .join("imports/body-01ARZ3NDEKTSV4RRFFQ69G5FAV/manifest.json")
            .is_file()
    );
    assert!(
        target
            .join("imports/20240101_120000/manifest.json")
            .is_file()
    );
    assert!(result.merge_summary.body_zones_differ.is_empty());
}

#[test]
fn a_zone_this_journal_already_holds_is_kept_and_a_different_one_is_counted_in_hours() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    body_window(&source, "20260110", "090000_3600", "r1");
    body_window(&source, "20260110", "100000_300", "r2");
    write(&source, APPLE_ZONE, NEW_YORK);
    write(&target, APPLE_ZONE, DENVER);

    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    assert_eq!(fs::read(target.join(APPLE_ZONE)).unwrap(), DENVER);
    // The windows merge anyway, and the summary says how much is on another clock.
    assert_eq!(result.merge_summary.body_zones_differ.len(), 1);
    assert_eq!(
        result.merge_summary.body_zones_differ[0].source,
        "Apple Health"
    );
    // An hour and five minutes of the clock: no more than that is claimed.
    assert_eq!(result.merge_summary.body_zones_differ[0].hours, 1);
}

/// The summary's line for windows arriving in New York's zone over Denver's.
fn zone_differs_after(windows: &[(&str, &str)], held: &[u8]) -> Vec<(String, u64)> {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    for (index, (day, key)) in windows.iter().enumerate() {
        body_window(&source, day, key, &format!("r{index}"));
    }
    write(&source, APPLE_ZONE, NEW_YORK);
    write(&target, APPLE_ZONE, held);
    let archive = archive_from(&source, &tree.path);
    merge_journal_archive(&archive, &target, &options(&tree), None)
        .unwrap()
        .merge_summary
        .body_zones_differ
        .into_iter()
        .map(|differs| (differs.source, differs.hours))
        .collect()
}

#[test]
fn hours_on_another_clock_count_each_stretch_once_and_a_day_score_none() {
    let line = zone_differs_after(
        &[
            ("20260110", "090000_3600"),
            ("20260110", "090000_3601"),
            ("20260110", "093000_300"),
            ("20260110", "000000_86400"),
        ],
        DENVER,
    );
    assert_eq!(line, [("Apple Health".to_owned(), 1)]);
}

#[test]
fn hours_on_another_clock_join_across_midnight() {
    let line = zone_differs_after(
        &[("20260110", "230000_7200"), ("20260111", "000000_3600")],
        DENVER,
    );
    assert_eq!(line, [("Apple Health".to_owned(), 2)]);
}

#[test]
fn less_than_an_hour_on_another_clock_is_still_said() {
    let line = zone_differs_after(&[("20260110", "090000_300")], DENVER);
    assert_eq!(line, [("Apple Health".to_owned(), 0)]);
}

#[test]
fn only_day_scores_on_another_clock_say_nothing() {
    assert!(zone_differs_after(&[("20260110", "000000_86400")], DENVER).is_empty());
}

#[test]
fn a_zone_this_journal_holds_but_cannot_read_is_not_called_different() {
    assert!(zone_differs_after(&[("20260110", "090000_3600")], b"not json").is_empty());
}

#[test]
fn the_same_zone_on_both_sides_says_nothing() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    body_window(&source, "20260110", "090000_3600", "r1");
    write(&source, APPLE_ZONE, DENVER);
    write(&target, APPLE_ZONE, DENVER);
    let archive = archive_from(&source, &tree.path);
    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert!(result.merge_summary.body_zones_differ.is_empty());
}

#[test]
fn a_body_window_landing_on_a_retired_card_steps_around_it_and_an_old_card_merges_as_ever() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    let card = "chronicle/20260110/import.apple_health/000000_300/day_summary_transcript.md";
    write(&target, card, b"# a retired card");
    body_window(&source, "20260110", "000000_300", "r1");
    write(&source, APPLE_ZONE, DENVER);
    // The archive's own card, on a day the target doesn't hold, comes in too.
    let other_card = "chronicle/20260111/import.apple_health/000000_300/day_summary_transcript.md";
    write(&source, other_card, b"# another retired card");

    let archive = archive_from(&source, &tree.path);
    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    assert!(target.join(card).is_file());
    assert!(
        target
            .join("chronicle/20260110/import.apple_health/000000_301/body.jsonl")
            .is_file()
    );
    assert!(target.join(other_card).is_file());
}

#[test]
fn a_journal_that_merged_the_same_hour_keeps_both_copies_on_the_grid() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    body_window(&source, "20260110", "090000_3600", "from-archive");
    body_window(&target, "20260110", "090000_3600", "from-here");
    write(&source, APPLE_ZONE, DENVER);
    write(&target, APPLE_ZONE, DENVER);

    let archive = archive_from(&source, &tree.path);
    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    let stream = target.join("chronicle/20260110/import.apple_health");
    assert!(stream.join("090000_3600/body.jsonl").is_file());
    assert!(stream.join("090000_3601/body.jsonl").is_file());
}

#[test]
fn an_exported_journal_carries_its_body_zone_through_a_merge() {
    use solstone_core_journal_archive::{ArchiveSource, EncodeArchiveRequest, encode_archive};
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    fs::create_dir_all(&target).unwrap();
    body_window(&source, "20260110", "090000_3600", "r1");
    write(&source, APPLE_ZONE, DENVER);
    let archive = tree.path.join("portable.zip");
    let opened = ArchiveSource::open(&source).unwrap();
    encode_archive(
        &EncodeArchiveRequest {
            source: &opened,
            solstone_version: "2.0.0",
            exported_at: "2026-01-01T00:00:00Z",
            day_window: None,
        },
        &mut File::create(&archive).unwrap(),
    )
    .unwrap();
    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert_eq!(fs::read(target.join(APPLE_ZONE)).unwrap(), DENVER);
}

#[test]
fn an_entity_merged_away_in_this_journal_refuses_the_import_and_changes_nothing() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &source,
        "sunstone",
        &json!({"id":"sunstone","name":"Zzyzx Tangent","type":"Project"}),
        None,
    )
    .unwrap();
    // The journal merged `sunstone` away; nothing else matches the name.
    fs::create_dir_all(target.join("entities")).unwrap();
    fs::write(
        target.join("entities/retired.json"),
        serde_json::to_vec(
            &json!({"ids":{"sunstone":{"state":"merged","dir":"sunstone","successor":"solstone"}}}),
        )
        .unwrap(),
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);
    let error = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap_err();
    assert!(error.to_string().contains("'solstone'"), "{error}");
    assert!(!target.join("entities/sunstone").exists());
    assert!(load_all_journal_entities(&target).unwrap().is_empty());
}

fn tombstone(target: &Path, id: &str) {
    fs::create_dir_all(target.join("entities")).unwrap();
    fs::write(
        target.join("entities/retired.json"),
        serde_json::to_vec(&json!({"ids":{id:{"state":"deleted","dir":id}}})).unwrap(),
    )
    .unwrap();
}

fn linked_with_note(root: &Path, facet: &str, folder: &str, entity_id: &str) {
    save_facet_entity_link(root, facet, folder, entity_id, &Map::new()).unwrap();
    add_observation(root, facet, folder, "a note", Some("2026-08-11"), None).unwrap();
}

#[test]
fn an_entity_deleted_here_is_held_and_its_links_are_left_out() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (id, name) in [("bob", "Qqxq Bob"), ("carol", "Qqxq Carol")] {
        save_entity_identity(
            &source,
            id,
            &json!({"id":id,"name":name,"type":"Person"}),
            None,
        )
        .unwrap();
    }
    // `work` exists here, so links merge one by one; `team` is new, so it is
    // copied whole.
    save_facet_entity_link(&target, "work", "existing", "existing", &Map::new()).unwrap();
    for facet in ["work", "team"] {
        linked_with_note(&source, facet, "bob", "bob");
        linked_with_note(&source, facet, "carol", "carol");
    }
    tombstone(&target, "bob");
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert!(!target.join("entities/bob").exists());
    assert!(target.join("entities/carol/entity.json").is_file());
    assert!(result.entity_dispositions.iter().any(|item| {
        item.source_id == "bob" && item.disposition == EntityDispositionKind::StagedDeletedHere
    }));
    for facet in ["work", "team"] {
        assert!(
            !target.join(format!("facets/{facet}/entities/bob")).exists(),
            "{facet}"
        );
        assert!(
            target
                .join(format!("facets/{facet}/entities/carol/observations.jsonl"))
                .is_file(),
            "{facet}"
        );
    }
    assert_eq!(result.merge_summary.entity_links_skipped, 2);
}

#[test]
fn a_link_to_a_deleted_id_follows_the_entity_it_matched_unless_the_facet_has_it() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &source,
        "bob",
        &json!({"id":"bob","name":"Qqxq Bob","type":"Person"}),
        None,
    )
    .unwrap();
    // Here, `bob` was deleted and the owner added "Qqxq Bob" again.
    save_entity_identity(
        &target,
        "bob_2",
        &json!({"id":"bob_2","name":"Qqxq Bob","type":"Person"}),
        None,
    )
    .unwrap();
    tombstone(&target, "bob");
    save_facet_entity_link(&target, "work", "existing", "existing", &Map::new()).unwrap();
    save_facet_entity_link(&target, "home", "bob_2", "bob_2", &Map::new()).unwrap();
    linked_with_note(&source, "work", "bob", "bob");
    linked_with_note(&source, "home", "bob", "bob");
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    // `work` had no link to bob_2: the archive's link becomes one, in the
    // folder named by bob_2's id, with its note.
    let link: Value = serde_json::from_slice(
        &fs::read(target.join("facets/work/entities/bob_2/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(link["entity_id"], "bob_2");
    assert!(!target.join("facets/work/entities/bob").exists());
    assert!(
        fs::read_to_string(target.join("facets/work/entities/bob_2/observations.jsonl"))
            .unwrap()
            .contains("a note")
    );
    // `home` already links bob_2: the note joins that link, no second one.
    assert!(!target.join("facets/home/entities/bob").exists());
    assert!(
        fs::read_to_string(target.join("facets/home/entities/bob_2/observations.jsonl"))
            .unwrap()
            .contains("a note")
    );
    assert_eq!(result.merge_summary.entity_links_skipped, 0);
    assert!(!target.join("entities/bob").exists());
}

#[test]
fn two_archive_links_to_a_deleted_id_become_one_link_to_the_entity_it_matched() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &source,
        "bob",
        &json!({"id":"bob","name":"Qqxq Bob","type":"Person"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &target,
        "bob_2",
        &json!({"id":"bob_2","name":"Qqxq Bob","type":"Person"}),
        None,
    )
    .unwrap();
    tombstone(&target, "bob");
    save_facet_entity_link(&target, "work", "existing", "existing", &Map::new()).unwrap();
    // The archive links `bob` from two folders.
    linked_with_note(&source, "work", "bob", "bob");
    linked_with_note(&source, "work", "robert", "bob");
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    let links: Vec<String> = fs::read_dir(target.join("facets/work/entities"))
        .unwrap()
        .filter_map(|entry| {
            let path = entry.unwrap().path().join("entity.json");
            let link: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
            Some(link["entity_id"].as_str()?.to_owned())
        })
        .filter(|id| id == "bob_2")
        .collect();
    assert_eq!(links.len(), 1);
    assert!(
        fs::read_to_string(target.join("facets/work/entities/bob_2/observations.jsonl"))
            .unwrap()
            .contains("a note")
    );
    assert_eq!(result.merge_summary.entity_links_skipped, 0);
}

#[test]
fn an_archives_entities_are_never_matched_against_each_other() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (id, name) in [("javier", "Javier"), ("javier_garcia", "Javier Garcia")] {
        save_entity_identity(
            &source,
            id,
            &json!({"id": id, "name": name, "type": "Person"}),
            None,
        )
        .unwrap();
    }
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert_eq!(
        result.merge_summary.entities_created, 2,
        "{:?}",
        result.entity_dispositions
    );
    assert_eq!(result.merge_summary.entities_staged, 0);
    let mut names: Vec<String> = load_all_journal_entities(&target)
        .unwrap()
        .into_iter()
        .map(|entity| entity.value["name"].as_str().unwrap().to_owned())
        .collect();
    names.sort();
    assert_eq!(names, ["Javier", "Javier Garcia"]);

    // The same archive again finds each entity it brought, and adds nothing.
    let again = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    assert_eq!(again.merge_summary.entities_created, 0);
    assert_eq!(again.merge_summary.entities_staged, 0);
    assert_eq!(load_all_journal_entities(&target).unwrap().len(), 2);
}

#[test]
fn a_link_follows_its_entity_into_the_one_it_matched_by_name() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    save_entity_identity(
        &source,
        "ada",
        &json!({"id":"ada","name":"Qqxq Ada","type":"Person"}),
        None,
    )
    .unwrap();
    // Here, "Qqxq Ada" is `countess`, and `ada` is somebody else.
    save_entity_identity(
        &target,
        "countess",
        &json!({"id":"countess","name":"Qqxq Ada","type":"Person"}),
        None,
    )
    .unwrap();
    save_entity_identity(
        &target,
        "ada",
        &json!({"id":"ada","name":"Wrbnkq Zorp","type":"Person"}),
        None,
    )
    .unwrap();
    // `work` is a facet here too; `fresh` arrives with the archive.
    save_facet_entity_link(&target, "work", "existing", "existing", &Map::new()).unwrap();
    linked_with_note(&source, "work", "ada", "ada");
    linked_with_note(&source, "fresh", "ada", "ada");
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    for facet in ["work", "fresh"] {
        let link: Value = serde_json::from_slice(
            &fs::read(target.join(format!("facets/{facet}/entities/countess/entity.json")))
                .unwrap(),
        )
        .unwrap();
        assert_eq!(link["entity_id"], "countess", "{facet}");
        assert!(
            !target.join(format!("facets/{facet}/entities/ada")).exists(),
            "{facet}: the archive's note must not reach this journal's `ada`"
        );
        assert_eq!(notes_at(&target, facet, "countess").len(), 1, "{facet}");
    }
    assert_eq!(result.merge_summary.entity_links_skipped, 0);
}

fn notes_at(root: &Path, facet: &str, folder: &str) -> Vec<Value> {
    fs::read_to_string(root.join(format!(
        "facets/{facet}/entities/{folder}/observations.jsonl"
    )))
    .unwrap()
    .lines()
    .map(|line| serde_json::from_str(line).unwrap())
    .collect()
}

fn with_ada(source: &Path, target: &Path) {
    for root in [source, target] {
        save_entity_identity(
            root,
            "ada",
            &json!({"id":"ada","name":"Qqxq Ada","type":"Person"}),
            None,
        )
        .unwrap();
    }
    save_facet_entity_link(target, "work", "existing", "existing", &Map::new()).unwrap();
}

#[test]
fn an_imported_link_keeps_the_notes_already_written_under_its_folder() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    with_ada(&source, &target);
    // Notes written here for the name before it was linked in this facet.
    for note in ["first here", "second here"] {
        add_observation(&target, "work", "ada", note, Some("2026-08-01"), None).unwrap();
    }
    linked_with_note(&source, "work", "ada", "ada");
    let archive = archive_from(&source, &tree.path);

    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    let link: Value = serde_json::from_slice(
        &fs::read(target.join("facets/work/entities/ada/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(link["entity_id"], "ada");
    let notes = notes_at(&target, "work", "ada");
    let contents: Vec<&str> = notes
        .iter()
        .map(|note| note["content"].as_str().unwrap())
        .collect();
    assert_eq!(contents, ["first here", "second here", "a note"]);
    let ids: std::collections::BTreeSet<u64> = notes
        .iter()
        .map(|note| note["id"].as_u64().unwrap())
        .collect();
    assert_eq!(ids.len(), notes.len());
}

#[test]
fn an_import_keeps_this_journals_link_fields_and_adds_nothing_twice() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    with_ada(&source, &target);
    save_facet_entity_link(
        &target,
        "work",
        "ada",
        "ada",
        json!({"detached": true}).as_object().unwrap(),
    )
    .unwrap();
    linked_with_note(&source, "work", "ada", "ada");
    let archive = archive_from(&source, &tree.path);

    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
    let after_first = notes_at(&target, "work", "ada");
    merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    let link: Value = serde_json::from_slice(
        &fs::read(target.join("facets/work/entities/ada/entity.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(link["detached"], true, "the owner removed it here");
    assert_eq!(notes_at(&target, "work", "ada"), after_first);
    assert_eq!(after_first.len(), 1);
}

#[test]
fn an_import_never_files_one_entitys_notes_under_another_sharing_its_folder() {
    let tree = TempTree::new();
    let source = tree.path.join("source");
    let target = tree.path.join("target");
    for (id, name) in [("robert", "Qqxq Robert"), ("bob", "Qqxq Bob")] {
        for root in [&source, &target] {
            save_entity_identity(
                root,
                id,
                &json!({"id":id,"name":name,"type":"Person"}),
                None,
            )
            .unwrap();
        }
    }
    // Here, robert's link sits in a folder named `bob`, as older builds wrote it.
    save_facet_entity_link(&target, "work", "bob", "robert", &Map::new()).unwrap();
    add_observation(
        &target,
        "work",
        "bob",
        "robert here",
        Some("2026-08-01"),
        None,
    )
    .unwrap();
    // The archive links both, robert's folder sorting first.
    save_facet_entity_link(&source, "work", "a_robert", "robert", &Map::new()).unwrap();
    add_observation(
        &source,
        "work",
        "a_robert",
        "about robert",
        Some("2026-08-02"),
        None,
    )
    .unwrap();
    save_facet_entity_link(&source, "work", "bob_jones", "bob", &Map::new()).unwrap();
    add_observation(
        &source,
        "work",
        "bob_jones",
        "about bob",
        Some("2026-08-03"),
        None,
    )
    .unwrap();
    let archive = archive_from(&source, &tree.path);

    let result = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();

    let robert =
        fs::read_to_string(target.join("facets/work/entities/bob/observations.jsonl")).unwrap();
    assert!(robert.contains("about robert"));
    assert!(!robert.contains("about bob"), "{robert}");
    assert_eq!(result.merge_summary.entity_links_skipped, 1);
}

#[test]
fn portable_export_and_merge_preserve_colliding_owner_names() {
    use solstone_core_journal_archive::{ArchiveSource, EncodeArchiveRequest, encode_archive};
    use std::io::Read;
    for selected_id in ["cache", "indexer"] {
        let tree = TempTree::new();
        let source = tree.path.join("source");
        let target = tree.path.join("target");
        let mut kept = Vec::new();
        for id in [selected_id] {
            for family in ["entities", "facets"] {
                let (name, bytes) = if family == "entities" {
                    (
                        format!("{family}/{id}/entity.json"),
                        json!({"id":id,"name":id,"type":"Person"})
                            .to_string()
                            .into_bytes(),
                    )
                } else {
                    (
                        format!("{family}/{id}/facet.json"),
                        json!({"name":id,"title":id}).to_string().into_bytes(),
                    )
                };
                kept.push((name, bytes));
            }
        }
        for name in [
            "imports/item/original.key",
            "imports/health-dedupe.sqlite-notes/original.key",
            "imports/health-dedupe.sqlite-wal/original.key",
            "imports/health-dedupe.sqlite/original.key",
            "chronicle/20260101/indexer/120001_60.sqlite/original.key",
            "imports/item/.removing",
            "imports/item/notes.removing_original",
            "imports/item/cache/owner.sqlite",
            "chronicle/20260101/indexer/120000_60/slides.key",
            "chronicle/20260101/source/120000_60/audio.jsonl.lock",
            "chronicle/20260101/source/120000_60/.tmp_123_4.tmp",
        ] {
            kept.push((name.to_owned(), b"owner bytes".to_vec()));
        }
        let omitted = [
            "indexer/journal.sqlite",
            ".removing_root/owner.json",
            "chronicle/20260101/.removing_flat/owner.json",
            "chronicle/20260101/source/.removing_qualified/owner.json",
            "imports/item/deep/.removing_deep/owner.json",
            "chronicle/20260101/120000_60/.removing_original",
            "imports/.health-dedupe.sqlite.rebuild-journal",
            "chronicle/20260101/indexer/indexer.sqlite",
            "health/brain-fingerprint.key",
            "config/.tmp_123_4.tmp",
        ];
        for (name, bytes) in kept
            .iter()
            .map(|(name, bytes)| (name.as_str(), bytes.as_slice()))
            .chain(
                omitted
                    .iter()
                    .map(|name| (*name, b"synthetic secret".as_slice())),
            )
        {
            let path = source.join(name);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, bytes).unwrap();
        }
        let credential_name = "mcp-endpoint/pop.ed25519.pk8";
        let source_credential = b"synthetic exported owner credential";
        let source_credential_path = source.join(credential_name);
        fs::create_dir_all(source_credential_path.parent().unwrap()).unwrap();
        fs::write(&source_credential_path, source_credential).unwrap();
        let archive = tree.path.join("portable.zip");
        let source = ArchiveSource::open(&source).unwrap();
        encode_archive(
            &EncodeArchiveRequest {
                source: &source,
                solstone_version: "2.0.0",
                exported_at: "2026-01-01T00:00:00Z",
                day_window: None,
            },
            &mut File::create(&archive).unwrap(),
        )
        .unwrap();
        let mut zip = zip::ZipArchive::new(File::open(&archive).unwrap()).unwrap();
        for (name, bytes) in &kept {
            let mut actual = Vec::new();
            zip.by_name(name).unwrap().read_to_end(&mut actual).unwrap();
            assert_eq!(&actual, bytes, "{name}");
        }
        let mut exported_credential = Vec::new();
        zip.by_name(credential_name)
            .unwrap()
            .read_to_end(&mut exported_credential)
            .unwrap();
        assert_eq!(exported_credential, source_credential);
        for name in omitted {
            assert!(zip.by_name(name).is_err(), "{name}");
        }
        drop(zip);
        let target_credential = target.join(credential_name);
        let receiving_credential = b"synthetic receiving owner credential";
        fs::create_dir_all(target_credential.parent().unwrap()).unwrap();
        fs::write(&target_credential, receiving_credential).unwrap();
        let merged = merge_journal_archive(&archive, &target, &options(&tree), None).unwrap();
        assert_eq!(merged.merge_summary.entities_created, 1, "{merged:?}");
        assert_eq!(fs::read(&target_credential).unwrap(), receiving_credential);
        assert_eq!(
            fs::read(&source_credential_path).unwrap(),
            source_credential
        );
        for (name, bytes) in &kept {
            let actual =
                fs::read(target.join(name)).unwrap_or_else(|error| panic!("{name}: {error}"));
            if name.starts_with("entities/") {
                let actual: Value = serde_json::from_slice(&actual).unwrap();
                assert_eq!(
                    actual["name"],
                    serde_json::from_slice::<Value>(bytes).unwrap()["name"]
                );
            } else if name.starts_with("facets/") {
                let actual: Value = serde_json::from_slice(&actual).unwrap();
                assert_eq!(
                    actual["title"],
                    serde_json::from_slice::<Value>(bytes).unwrap()["title"]
                );
            } else {
                assert_eq!(&actual, bytes, "{name}");
            }
        }
    }
}
