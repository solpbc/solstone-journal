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
use solstone_core_facets::{save_facet_entity_link, save_observations};
use solstone_core_import_sources::archive::{
    ArchiveMergeOptions, EntityDispositionKind, PrincipalAdoption, RetryDisposition,
    SegmentDispositionKind, merge_journal_archive,
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
fn identical_retry_is_noop_and_different_segment_is_enumerated() {
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
    fs::write(source.join("chronicle/20260811/120000_60/value"), b"two").unwrap();
    let conflict_archive = archive_from(&source, &tree.path);
    let conflict = merge_journal_archive(&conflict_archive, &target, &options, None).unwrap();
    assert!(
        conflict
            .segment_dispositions
            .iter()
            .any(|item| item.day == "20260811"
                && item.stream == "_default"
                && item.key == "120000_60"
                && item.disposition == SegmentDispositionKind::DifferingContentCollision)
    );
    assert_eq!(
        fs::read(target.join("chronicle/20260811/120000_60/value")).unwrap(),
        before
    );
    assert_eq!(fs::read(&archive).unwrap(), archive_before);
    assert_eq!(
        fs::metadata(&archive).unwrap().modified().unwrap(),
        archive_mtime
    );
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
        ImportSourcesError::MergePublishFailed { detail } => {
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
    save_observations(
        &source,
        "work",
        "from-archive",
        &[json!({"content":"from archive", "observed_at":"2026-08-11"})],
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
    assert_eq!(family_snapshot(&target), before);
    assert!(!target.join("chronicle/20260311").exists());
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
