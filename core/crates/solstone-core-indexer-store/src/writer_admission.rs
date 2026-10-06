// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Advisory writer lease for index mutations.
//!
//! Production admission topology and lock ordering:
//!
//! Admit (acquires index writer lease):
//! - `scan_journal` (scan)
//! - `rescan_file` (rescan-file)
//! - `reset_index` (reset)
//! - `prune_by_paths` (prune-paths)
//! - `prune_authored_chat_paths` (prune-authored-chat)
//! - `prune_chunks_by_stream` (prune-stream)
//! - `rebuild_edges` (rebuild-edges)
//! - `fold_entity_edges_for_recorded_merge` (fold-entity-edges)
//! - `rebuild_edges_for_recorded_merge_undo` (rebuild-edges-fingerprint)
//! - `apply_path_lookup` (path-lookup)
//! - `apply_classification_batch` (classification-batch, once per batch)
//! - `reconcile_stale_classifications` (reconcile-classifications)
//! - `migrate_index_stream` rebuild arm (migrate-index-stream)
//! - `remove_legacy_index_artifacts` (remove-legacy-index)
//!
//! Inside those callers, no second lease:
//! - journal-cli indexer verbs and `LocalScanReindex::request_full_reindex`
//! - owner `facet_merge_transaction_in_journal` scan+reconcile
//! - doctor `scan_journal` after the outer trust guard drops
//! - `solstone-core` indexer maintenance/classifications/path-lookup/prune/fold/fingerprint verbs
//! - talent `attempt_saved_publication`, `append_day_record_with_sources`, weekly reflection rescan, daily execution while `DailyUnitAuthority` is held
//! - think `write_sense_and_change` / `NativeIndexBoundary` while `segment_turn` is held
//! - import `NativePublicationOperations::rescan_file` and `finish_import_attempt_with` while `imports/<id>.lock` is held
//! - backup `NativeJournalMaintenance::full_scan`
//! - segment move prune+rescan
//! - retention `RetentionIndex::paths_removed` via `notify_index` (transcripts delete, strava delete while `imports/.strava` is held, `release_misplaced_strava_pieces`)
//!
//! Read-only, no lease:
//! - `open_index_reader`
//! - `inspect_classifications`
//! - `inspect_path_lookup`
//! - `fingerprint_edge_rows`
//! - `migrate_index_stream` probe and dry-run
//! - indexer-query search
//! - health-web `evaluate_search_index`
//! - MCP recall
//!
//! Fixture-only:
//! - public `open_index` (seeds a writable connection and does not take this lease)
//!
//! Lock Order:
//! Doctor file mutation is entity/facet trust, then release, then this lease.
//! Reconciliation is the facet-reconcile single-flight lock, then this lease, then entity/facet trust for the snapshot only.
//! Daily-unit, segment-turn, `imports/.strava`, and `imports/<id>.lock` stay outside and before this lease.
//! Nested prune, classify, and rebuild take `&IndexAdmission` and must not call `hold_lock`.
//! Drop releases the kernel lock and does not unlink `indexer/journal.sqlite.lock`.
//! Reset and legacy artifact removal leave that sidecar in place.

use std::path::Path;
use std::time::{Duration, Instant};

use solstone_core_journal_io::errors::LockError;
use solstone_core_journal_io::locking::{FileLock, LockOptions, hold_lock};

use crate::StoreError;
use crate::db::db_path;

pub(crate) struct IndexAdmission {
    _lock: FileLock,
    operation: &'static str,
    wait: Duration,
    admitted_at: Instant,
}

impl IndexAdmission {
    pub(crate) fn acquire(journal: &Path, operation: &'static str) -> Result<Self, StoreError> {
        let started = Instant::now();
        let target_path = db_path(journal);
        match hold_lock(&target_path, LockOptions::default()) {
            Ok(lock) => {
                let wait = started.elapsed();
                let admission = Self {
                    _lock: lock,
                    operation,
                    wait,
                    admitted_at: Instant::now(),
                };
                #[cfg(all(test, feature = "full-tests"))]
                {
                    if operation == "scan"
                        && let Some(seam) = std::env::var_os("SOLSTONE_TEST_INDEX_WRITER_SEAM")
                    {
                        let seam_dir = std::path::PathBuf::from(seam);
                        let _ = std::fs::write(seam_dir.join("scan-admitted"), b"admitted");
                    }
                    check_test_seam("hold");
                }
                Ok(admission)
            }
            Err(LockError::Timeout(timeout)) => {
                let wait = started.elapsed();
                let cause = timeout.to_string();
                log::warn!(
                    target: "solstone::indexer",
                    "index writer busy operation={operation} wait_ms={} cause={cause}",
                    wait.as_millis()
                );
                Err(StoreError::WriterBusy { operation, cause })
            }
            Err(LockError::Io { source, .. }) => {
                let wait = started.elapsed();
                log::warn!(
                    target: "solstone::indexer",
                    "index writer admission failed operation={operation} wait_ms={} cause={source}",
                    wait.as_millis()
                );
                Err(StoreError::Io(source))
            }
        }
    }
}

impl Drop for IndexAdmission {
    fn drop(&mut self) {
        let hold = self.admitted_at.elapsed();
        log::info!(
            target: "solstone::indexer",
            "index writer released operation={} wait_ms={} hold_ms={}",
            self.operation,
            self.wait.as_millis(),
            hold.as_millis()
        );
    }
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn check_test_seam(pause_kind: &str) {
    let Some(seam_var) = std::env::var_os("SOLSTONE_TEST_INDEX_WRITER_SEAM") else {
        return;
    };
    let Some(expected_pause) = std::env::var_os("SOLSTONE_TEST_INDEX_WRITER_PAUSE") else {
        return;
    };
    if expected_pause.to_string_lossy() != pause_kind {
        return;
    }
    let seam_dir = std::path::PathBuf::from(seam_var);
    let ready_file = seam_dir.join(format!("{pause_kind}.ready"));
    let release_file = seam_dir.join(format!("{pause_kind}.release"));
    let _ = std::fs::write(&ready_file, b"ready");

    if pause_kind == "hold" {
        loop {
            std::thread::sleep(Duration::from_millis(50));
        }
    }

    let deadline = Instant::now() + Duration::from_secs(30);
    while !release_file.is_file() {
        assert!(
            Instant::now() < deadline,
            "test seam release timed out for {pause_kind}"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(not(all(test, feature = "full-tests")))]
#[inline(always)]
pub(crate) fn check_test_seam(_pause_kind: &str) {}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::chunk_sources::{apply_path_lookup, inspect_path_lookup};
    use crate::classification_batch::{
        apply_classification_batch, drain_classifications, inspect_classifications,
    };
    use crate::db::{
        open_index_reader, prune_authored_chat_paths, prune_by_paths, prune_chunks_by_stream,
        reset_index,
    };
    use crate::merge::fingerprint_edge_rows;
    use crate::migrations::index_stream::{legacy_index_artifacts, remove_legacy_index_artifacts};
    use crate::test_support::reserve_temp_path;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-store-admission-{name}"))
    }

    #[test]
    fn writer_busy_display_format_and_contents() {
        let err = StoreError::WriterBusy {
            operation: "rescan-file",
            cause: "could not acquire lock for /path/to/indexer/journal.sqlite within 10s".into(),
        };
        let display = err.to_string();
        assert_eq!(
            display,
            "index writer busy for rescan-file: could not acquire lock for /path/to/indexer/journal.sqlite within 10s"
        );
        assert!(!display.contains("indexed"));
        assert!(!display.contains("declined"));
    }

    #[test]
    fn missing_journal_read_only_and_probes_do_not_create_database_or_dir() {
        let root = temp_root("missing-reads");
        assert!(!root.exists());

        let fp = fingerprint_edge_rows(&root);
        assert!(matches!(fp, Err(StoreError::MissingFile(_))));
        assert!(!root.join("indexer").exists());

        let cls = inspect_classifications(&root).expect("inspect classifications");
        assert_eq!(
            cls.initialization,
            crate::ClassificationInitialization::Absent
        );
        assert!(!root.join("indexer").exists());

        let pl = inspect_path_lookup(&root).expect("inspect path lookup");
        assert!(!pl.ready);
        assert!(!root.join("indexer").exists());

        let rdr = open_index_reader(&root);
        assert!(matches!(rdr, Err(StoreError::MissingFile(_))));
        assert!(!root.join("indexer").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn missing_journal_prunes_do_not_create_journal_sqlite() {
        let root = temp_root("missing-prunes");
        assert!(!root.exists());

        let by_paths = prune_by_paths(&root, &["20260101/default/100000_300/note.txt"])
            .expect("prune by paths");
        assert_eq!(by_paths, None);
        assert!(!root.join("indexer").join("journal.sqlite").exists());

        let authored = prune_authored_chat_paths(&root).expect("prune authored chat");
        assert_eq!(authored, None);
        assert!(!root.join("indexer").join("journal.sqlite").exists());

        let stream = prune_chunks_by_stream(&root, "default").expect("prune stream");
        assert_eq!(stream.chunks, 0);
        assert_eq!(stream.files, 0);
        assert!(!root.join("indexer").join("journal.sqlite").exists());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn legacy_artifacts_and_reset_leave_lock_sidecar_intact() {
        let root = temp_root("legacy-sidecar");
        let index_dir = root.join("indexer");
        fs::create_dir_all(&index_dir).expect("create index dir");

        let lock_path = index_dir.join("journal.sqlite.lock");
        fs::write(&lock_path, b"keep").expect("write lock sidecar");

        let artifacts = legacy_index_artifacts(&root);
        for artifact in &artifacts {
            assert!(
                !artifact.to_string_lossy().ends_with(".lock"),
                "legacy_index_artifacts must not name .lock sidecar"
            );
        }

        reset_index(&root).expect("reset index");
        assert_eq!(
            fs::read(&lock_path).expect("read lock sidecar after reset"),
            b"keep"
        );

        fs::write(index_dir.join("journal.sqlite"), b"sqlite").expect("write sqlite");
        fs::write(index_dir.join("journal.sqlite-wal"), b"wal").expect("write wal");
        fs::write(index_dir.join("journal.sqlite-shm"), b"shm").expect("write shm");

        let report = remove_legacy_index_artifacts(&root).expect("remove legacy artifacts");
        assert_eq!(report.deleted(), 3);
        assert!(!index_dir.join("journal.sqlite").exists());
        assert!(!index_dir.join("journal.sqlite-wal").exists());
        assert!(!index_dir.join("journal.sqlite-shm").exists());
        assert_eq!(
            fs::read(&lock_path).expect("read lock sidecar after removal"),
            b"keep"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn stable_item_signatures_are_referenced() {
        let _inspect_cls =
            inspect_classifications as fn(&Path) -> Result<crate::ClassificationStatus, StoreError>;
        let _apply_cls = apply_classification_batch
            as fn(&Path) -> Result<crate::ClassificationStatus, StoreError>;
        let _drain_cls =
            drain_classifications as fn(&Path) -> Result<crate::ClassificationStatus, StoreError>;
        let _inspect_pl =
            inspect_path_lookup as fn(&Path) -> Result<crate::PathLookupStatus, StoreError>;
        let _apply_pl =
            apply_path_lookup as fn(&Path) -> Result<crate::PathLookupStatus, StoreError>;
        let _open_rdr = open_index_reader as fn(&Path) -> Result<rusqlite::Connection, StoreError>;
        let _classify_batch = crate::classification_batch::classify_one_batch;
        let _attempt_pub: fn(
            &Path,
            &Path,
            fn(&Path, &Path) -> Result<crate::scan::RescanFileStatus, String>,
        ) -> crate::scan::SavedPublicationAttempt = crate::scan::attempt_saved_publication;
    }

    #[cfg(all(test, feature = "full-tests"))]
    mod full_process_tests {
        use super::*;
        use crate::db::open_index;
        use crate::scan::rescan_file;
        use std::process::Command;

        fn run_child_role(
            test_name: &str,
            role: &str,
            journal: &Path,
            seam_dir: &Path,
            pause: &str,
        ) -> std::process::Child {
            let exe = std::env::current_exe().expect("current exe");
            Command::new(exe)
                .arg("--exact")
                .arg(test_name)
                .arg("--nocapture")
                .env("SOLSTONE_TEST_CHILD_ROLE", role)
                .env("SOLSTONE_TEST_JOURNAL", journal)
                .env("SOLSTONE_TEST_INDEX_WRITER_SEAM", seam_dir)
                .env("SOLSTONE_TEST_INDEX_WRITER_PAUSE", pause)
                .spawn()
                .expect("spawn child process")
        }

        fn wait_for_file(path: &Path) {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !path.is_file() {
                assert!(
                    Instant::now() < deadline,
                    "wait for file timed out: {}",
                    path.display()
                );
                std::thread::sleep(Duration::from_millis(10));
            }
        }

        #[test]
        fn two_writers_exclude_each_other_before_source_and_cursor_reads() {
            let role = std::env::var("SOLSTONE_TEST_CHILD_ROLE").unwrap_or_default();
            if role == "child_a" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let note = journal.join("chronicle/20260717/default/100000_300/talents/audio.md");
                fs::create_dir_all(note.parent().unwrap()).unwrap();
                fs::write(&note, "# Audio").unwrap();
                let res = rescan_file(&journal, &note);
                let _ = res;
                return;
            } else if role == "child_b" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let res = apply_classification_batch(&journal);
                let _ = res;
                return;
            }

            let root =
                std::env::temp_dir().join(format!("solstone-test-excl-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            // Test A: Child A in rescan_file pauses at before-source
            let mut child_a = run_child_role(
                "writer_admission::tests::two_writers_exclude_each_other_before_source_and_cursor_reads",
                "child_a",
                &root,
                &seam,
                "before-source",
            );
            wait_for_file(&seam.join("before-source.ready"));

            let note_path = root.join("chronicle/20260717/default/100000_300/talents/audio.md");
            fs::create_dir_all(note_path.parent().unwrap()).unwrap();
            fs::write(&note_path, "# Audio").unwrap();
            let parent_rescan = rescan_file(&root, &note_path);
            match parent_rescan {
                Err(StoreError::WriterBusy { operation, .. }) => {
                    assert_eq!(operation, "rescan-file");
                }
                other => panic!("expected WriterBusy on rescan-file, got {other:?}"),
            }
            fs::write(seam.join("before-source.release"), b"go").unwrap();
            let _ = child_a.wait().unwrap();

            // Test B: Child B in apply_classification_batch pauses at before-cursor
            fs::remove_file(seam.join("before-source.ready")).unwrap();
            fs::remove_file(seam.join("before-source.release")).unwrap();
            let _conn = open_index(&root).unwrap();
            drop(_conn);

            let mut child_b = run_child_role(
                "writer_admission::tests::two_writers_exclude_each_other_before_source_and_cursor_reads",
                "child_b",
                &root,
                &seam,
                "before-cursor",
            );
            wait_for_file(&seam.join("before-cursor.ready"));

            let parent_batch = apply_classification_batch(&root);
            match parent_batch {
                Err(StoreError::WriterBusy { operation, .. }) => {
                    assert_eq!(operation, "classification-batch");
                }
                other => panic!("expected WriterBusy on classification-batch, got {other:?}"),
            }
            fs::write(seam.join("before-cursor.release"), b"go").unwrap();
            let _ = child_b.wait().unwrap();

            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn process_death_releases_the_lease_and_reacquire_keeps_the_sidecar() {
            let role = std::env::var("SOLSTONE_TEST_CHILD_ROLE").unwrap_or_default();
            if role == "child_hold" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let _adm = IndexAdmission::acquire(&journal, "scan").unwrap();
                return;
            }

            let root =
                std::env::temp_dir().join(format!("solstone-test-death-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            let mut child = run_child_role(
                "writer_admission::tests::process_death_releases_the_lease_and_reacquire_keeps_the_sidecar",
                "child_hold",
                &root,
                &seam,
                "hold",
            );
            wait_for_file(&seam.join("hold.ready"));
            let sidecar = root.join("indexer/journal.sqlite.lock");
            assert!(sidecar.exists(), "sidecar must exist while lease is held");

            child.kill().expect("kill child holding lease");
            let _ = child.wait();

            let prune_res = prune_by_paths(&root, &["dummy"]);
            assert!(
                prune_res.is_ok(),
                "prune after killed child must succeed, got {prune_res:?}"
            );
            assert!(sidecar.exists(), "sidecar must survive reacquisition");

            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn memory_rescan_paused_before_database_creation_makes_prune_wait() {
            let role = std::env::var("SOLSTONE_TEST_CHILD_ROLE").unwrap_or_default();
            if role == "child_memory" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let note = journal.join("chronicle/20260717/agent-memory-0000000000000000000000000000000000000000000000000000000000000001/100000_300/note.txt");
                let res = rescan_file(&journal, &note);
                match res {
                    Ok(crate::scan::RescanFileStatus::Indexed { .. }) => std::process::exit(0),
                    other => {
                        eprintln!("memory rescan child failed: {other:?}");
                        std::process::exit(1);
                    }
                }
            }

            let root = std::env::temp_dir()
                .join(format!("solstone-test-mem-prune-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            let source = solstone_core_format::agent_memory::SourceKey::from_verified_id(
                "0000000000000000000000000000000000000000000000000000000000000001",
            );
            let coordinate = solstone_core_format::agent_memory::Coordinate {
                day: "20260717".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "100000_300".into(),
            };
            let seg_dir = root
                .join("chronicle")
                .join(&coordinate.day)
                .join(&coordinate.stream)
                .join(&coordinate.segment);
            fs::create_dir_all(&seg_dir).unwrap();
            let note_path = seg_dir.join("note.txt");
            let note_rel = format!(
                "chronicle/{}/{}/{}/note.txt",
                coordinate.day, coordinate.stream, coordinate.segment
            );
            let bytes = b"original memory content";
            let record: solstone_core_format::agent_memory::OperationRecord =
                serde_json::from_value(serde_json::json!({
                    "operation_id": "operation-1",
                    "digest": solstone_core_format::agent_memory::digest(bytes),
                    "byte_count": bytes.len(),
                    "created_at": "2026-07-17T10:00:00Z",
                    "origin_kind": "agent_memory",
                    "creation_label": "connection label",
                    "coordinate": coordinate,
                    "phase": "chained",
                    "chain": {"prev_day": null, "prev_segment": null, "seq": 1}
                }))
                .expect("valid operation record");
            let ready =
                solstone_core_format::agent_memory::ready_document(&record, &source).unwrap();
            fs::write(&note_path, bytes).unwrap();
            fs::write(
                seg_dir.join("origin.json"),
                serde_json::to_vec(&ready.origin).unwrap(),
            )
            .unwrap();
            fs::write(
                seg_dir.join("ready.json"),
                serde_json::to_vec(&ready).unwrap(),
            )
            .unwrap();
            fs::write(
                seg_dir.join("stream.json"),
                serde_json::json!({
                    "stream": &coordinate.stream,
                    "prev_day": null,
                    "prev_segment": null,
                    "seq": 1
                })
                .to_string(),
            )
            .unwrap();

            let mut child = run_child_role(
                "writer_admission::tests::memory_rescan_paused_before_database_creation_makes_prune_wait",
                "child_memory",
                &root,
                &seam,
                "after-memory-read",
            );
            wait_for_file(&seam.join("after-memory-read.ready"));
            assert!(
                !root.join("indexer/journal.sqlite").exists(),
                "database must not exist yet at after-memory-read pause"
            );

            let root_clone = root.clone();
            let note_rel_prune = note_rel.clone();
            let prune_handle =
                std::thread::spawn(move || prune_by_paths(&root_clone, &[&note_rel_prune]));

            // Assert for up to 500ms that prune has not finished because it is blocked behind rescan lease
            let check_start = Instant::now();
            while check_start.elapsed() < Duration::from_millis(500) {
                assert!(
                    !prune_handle.is_finished(),
                    "prune must not complete while child holds admission"
                );
                std::thread::sleep(Duration::from_millis(20));
            }

            fs::write(seam.join("after-memory-read.release"), b"go").unwrap();
            let status = child.wait().unwrap();
            assert!(status.success(), "child memory rescan must succeed");

            let prune_res = prune_handle
                .join()
                .expect("join prune thread")
                .expect("prune succeeded");
            assert!(
                prune_res.is_some(),
                "prune must return Some counts after removing the newly indexed row"
            );

            let ro_conn = open_index_reader(&root).expect("open index reader");
            let original_count: i64 = ro_conn
                .query_row(
                    "SELECT count(*) FROM memory_originals WHERE path=?",
                    [&note_rel],
                    |row| row.get(0),
                )
                .expect("query memory_originals");
            assert_eq!(
                original_count, 0,
                "memory_originals row must be removed by prune"
            );

            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn completed_prune_rejects_a_later_rescan_of_the_removed_original() {
            let root = temp_root("completed-prune");
            let source = solstone_core_format::agent_memory::SourceKey::from_verified_id(
                "0000000000000000000000000000000000000000000000000000000000000001",
            );
            let coordinate = solstone_core_format::agent_memory::Coordinate {
                day: "20260717".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "100000_300".into(),
            };
            let seg_dir = root
                .join("chronicle")
                .join(&coordinate.day)
                .join(&coordinate.stream)
                .join(&coordinate.segment);
            fs::create_dir_all(&seg_dir).unwrap();
            let note_path = seg_dir.join("note.txt");
            let note_rel = format!(
                "chronicle/{}/{}/{}/note.txt",
                coordinate.day, coordinate.stream, coordinate.segment
            );
            let bytes = b"original memory content";
            let record: solstone_core_format::agent_memory::OperationRecord =
                serde_json::from_value(serde_json::json!({
                    "operation_id": "operation-1",
                    "digest": solstone_core_format::agent_memory::digest(bytes),
                    "byte_count": bytes.len(),
                    "created_at": "2026-07-17T10:00:00Z",
                    "origin_kind": "agent_memory",
                    "creation_label": "connection label",
                    "coordinate": coordinate,
                    "phase": "chained",
                    "chain": {"prev_day": null, "prev_segment": null, "seq": 1}
                }))
                .expect("valid operation record");
            let ready =
                solstone_core_format::agent_memory::ready_document(&record, &source).unwrap();
            fs::write(&note_path, bytes).unwrap();
            fs::write(
                seg_dir.join("origin.json"),
                serde_json::to_vec(&ready.origin).unwrap(),
            )
            .unwrap();
            fs::write(
                seg_dir.join("ready.json"),
                serde_json::to_vec(&ready).unwrap(),
            )
            .unwrap();
            fs::write(
                seg_dir.join("stream.json"),
                serde_json::json!({
                    "stream": &coordinate.stream,
                    "prev_day": null,
                    "prev_segment": null,
                    "seq": 1
                })
                .to_string(),
            )
            .unwrap();

            let initial_rescan = rescan_file(&root, &note_path).expect("initial rescan succeeds");
            assert!(matches!(
                initial_rescan,
                crate::scan::RescanFileStatus::Indexed { .. }
            ));

            fs::remove_file(&note_path).unwrap();
            let prune_res = prune_by_paths(&root, &[&note_rel]).expect("prune succeeds");
            assert!(prune_res.is_some());

            let later_rescan = rescan_file(&root, &note_path);
            assert!(matches!(later_rescan, Err(StoreError::MissingFile(_))));

            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn classification_batch_releases_admission_before_the_post_commit_barrier() {
            let role = std::env::var("SOLSTONE_TEST_CHILD_ROLE").unwrap_or_default();
            if role == "child_cls_barrier" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let _ = apply_classification_batch(&journal);
                return;
            }

            let root =
                std::env::temp_dir().join(format!("solstone-test-cls-rel-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            let conn = open_index(&root).unwrap();
            drop(conn);

            let mut child = run_child_role(
                "writer_admission::tests::classification_batch_releases_admission_before_the_post_commit_barrier",
                "child_cls_barrier",
                &root,
                &seam,
                "after-classification-release",
            );
            wait_for_file(&seam.join("after-classification-release.ready"));

            let prune_res = prune_by_paths(&root, &["dummy"]);
            assert!(
                prune_res.is_ok(),
                "parent must be able to acquire writer lease while child is at post-commit barrier, got {prune_res:?}"
            );

            fs::write(seam.join("after-classification-release.release"), b"go").unwrap();
            let _ = child.wait().unwrap();

            let _ = fs::remove_dir_all(&root);
        }
    }
}
