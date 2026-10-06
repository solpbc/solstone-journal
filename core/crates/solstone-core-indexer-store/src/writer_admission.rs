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
//! - `migrate_index_stream` non-dry-run operation (migrate-index-stream)
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
//! - `migrate_index_stream` dry-run
//! - indexer-query search
//! - health-web `evaluate_search_index`
//! - MCP recall
//!
//! Fixture-only:
//! - `open_index` exists only in unit-test builds or with `test-fixtures`.
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
                #[cfg(any(all(test, feature = "full-tests"), feature = "test-hooks"))]
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
                Err(StoreError::WriterAdmission {
                    operation,
                    cause: source.to_string(),
                })
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

#[cfg(any(all(test, feature = "full-tests"), feature = "test-hooks"))]
pub(crate) fn check_test_seam(pause_kind: &str) {
    let Some(seam_var) = std::env::var_os("SOLSTONE_TEST_INDEX_WRITER_SEAM") else {
        return;
    };
    let seam_dir = std::path::PathBuf::from(seam_var);
    std::fs::write(seam_dir.join(format!("{pause_kind}.observed")), b"observed")
        .expect("write test operation witness");
    let Some(expected_pause) = std::env::var_os("SOLSTONE_TEST_INDEX_WRITER_PAUSE") else {
        return;
    };
    if expected_pause.to_string_lossy() != pause_kind {
        return;
    }
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

#[cfg(not(any(all(test, feature = "full-tests"), feature = "test-hooks")))]
#[inline(always)]
pub(crate) fn check_test_seam(_pause_kind: &str) {}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) mod tests {
    use super::*;
    use crate::chunk_sources::inspect_path_lookup;
    use crate::classification_batch::{apply_classification_batch, inspect_classifications};
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
    fn admission_io_failure_reports_the_operation_without_changing_source() {
        let root = temp_root("admission-io");
        fs::create_dir_all(&root).unwrap();
        fs::write(root.join("indexer"), b"occupied").unwrap();
        let error = reset_index(&root).unwrap_err();
        assert!(matches!(
            &error,
            StoreError::WriterAdmission { operation: "reset", cause } if !cause.is_empty()
        ));
        assert!(error.to_string().contains("reset"));
        assert_eq!(fs::read(root.join("indexer")).unwrap(), b"occupied");
        fs::remove_dir_all(root).unwrap();
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

    #[cfg(all(test, feature = "full-tests"))]
    mod full_process_tests {
        use super::*;
        use crate::db::open_index;
        use crate::scan::rescan_file;
        use std::process::Command;

        struct TestChild {
            child: std::process::Child,
            log_path: PathBuf,
            test_name: String,
            killed: bool,
        }

        impl TestChild {
            fn wait(&mut self) -> std::io::Result<std::process::ExitStatus> {
                let deadline = Instant::now() + Duration::from_secs(30);
                loop {
                    if let Some(status) = self.child.try_wait()? {
                        if self.killed {
                            return Ok(status);
                        }
                        let output = fs::read_to_string(&self.log_path)?;
                        assert!(status.success(), "child test failed: {output}");
                        assert_eq!(
                            output
                                .matches("test result: ok. 1 passed; 0 failed;")
                                .count(),
                            1,
                            "child must execute exactly one test: {output}"
                        );
                        assert!(
                            output.contains(&format!("test {} ... ok", self.test_name)),
                            "selected child test must pass: {output}"
                        );
                        return Ok(status);
                    }
                    if Instant::now() >= deadline {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "test child did not finish",
                        ));
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
            }

            fn kill(&mut self) -> std::io::Result<()> {
                self.child.kill()?;
                self.killed = true;
                Ok(())
            }
        }

        impl Drop for TestChild {
            fn drop(&mut self) {
                let _ = self.child.kill();
                let _ = self.child.wait();
            }
        }

        fn memory_note_relative() -> String {
            let source = solstone_core_format::agent_memory::SourceKey::from_verified_id(
                "writer-test-owner",
            );
            format!(
                "20260717/agent-memory-{}/100000_300/note.txt",
                source.component()
            )
        }

        fn seed_memory_original(root: &Path) -> (PathBuf, String) {
            use solstone_core_format::agent_memory::{
                Coordinate, OperationRecord, SourceKey, digest, ready_document,
            };
            let source = SourceKey::from_verified_id("writer-test-owner");
            let coordinate = Coordinate {
                day: "20260717".into(),
                stream: format!("agent-memory-{}", source.component()),
                segment: "100000_300".into(),
            };
            let rel = memory_note_relative();
            let note = root.join("chronicle").join(&rel);
            let segment = note.parent().unwrap();
            fs::create_dir_all(segment).unwrap();
            let bytes = b"original memory content";
            let record: OperationRecord = serde_json::from_value(serde_json::json!({
                "operation_id": "operation-1",
                "digest": digest(bytes),
                "byte_count": bytes.len(),
                "created_at": "2026-07-17T10:00:00Z",
                "origin_kind": "agent_memory",
                "creation_label": "connection label",
                "coordinate": coordinate,
                "phase": "chained",
                "chain": {"prev_day": null, "prev_segment": null, "seq": 1}
            }))
            .unwrap();
            let ready = ready_document(&record, &source).unwrap();
            fs::write(&note, bytes).unwrap();
            fs::write(
                segment.join("origin.json"),
                serde_json::to_vec(&ready.origin).unwrap(),
            )
            .unwrap();
            fs::write(
                segment.join("ready.json"),
                serde_json::to_vec(&ready).unwrap(),
            )
            .unwrap();
            fs::write(
                segment.join("stream.json"),
                serde_json::json!({
                    "stream": &coordinate.stream, "prev_day": null, "prev_segment": null, "seq": 1
                })
                .to_string(),
            )
            .unwrap();
            drop(hold_lock(segment, LockOptions::default()).unwrap());
            assert!(
                matches!(
                    solstone_core_memory_original::read_original(root, &source, &coordinate),
                    solstone_core_memory_original::OriginalRead::Ready { .. }
                ),
                "fixture must be a published, readable original"
            );
            (note, rel)
        }

        fn run_child_role(
            test_name: &str,
            role: &str,
            journal: &Path,
            seam_dir: &Path,
            pause: &str,
        ) -> TestChild {
            let exe = std::env::current_exe().expect("current exe");
            let log_path = seam_dir.join(format!("{role}.child.log"));
            let output = fs::File::create(&log_path).unwrap();
            TestChild {
                child: Command::new(exe)
                    .arg("--exact")
                    .arg(test_name)
                    .arg("--nocapture")
                    .env("SOLSTONE_TEST_CHILD_ROLE", role)
                    .env("SOLSTONE_TEST_JOURNAL", journal)
                    .env("SOLSTONE_TEST_INDEX_WRITER_SEAM", seam_dir)
                    .env("SOLSTONE_TEST_INDEX_WRITER_PAUSE", pause)
                    .stdout(output.try_clone().unwrap())
                    .stderr(output)
                    .spawn()
                    .expect("spawn child process"),
                log_path,
                test_name: test_name.to_owned(),
                killed: false,
            }
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
            if role == "blocked_source" || role == "blocked_cursor" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let error = if role == "blocked_source" {
                    rescan_file(
                        &journal,
                        &journal.join("chronicle/20260717/default/100000_300/talents/audio.md"),
                    )
                    .unwrap_err()
                } else {
                    apply_classification_batch(&journal).unwrap_err()
                };
                let expected = if role == "blocked_source" {
                    "rescan-file"
                } else {
                    "classification-batch"
                };
                assert!(
                    matches!(error, StoreError::WriterBusy { operation, .. } if operation == expected)
                );
                return;
            }
            if role == "child_a" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let note = journal.join("chronicle/20260717/default/100000_300/talents/audio.md");
                fs::create_dir_all(note.parent().unwrap()).unwrap();
                fs::write(&note, "# Audio").unwrap();
                assert!(matches!(
                    rescan_file(&journal, &note).unwrap(),
                    crate::scan::RescanFileStatus::Indexed { .. }
                ));
                return;
            } else if role == "child_b" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                apply_classification_batch(&journal).unwrap();
                return;
            }

            let root =
                std::env::temp_dir().join(format!("solstone-test-excl-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            // Test A: Child A in rescan_file pauses at before-source
            let mut child_a = run_child_role(
                "writer_admission::tests::full_process_tests::two_writers_exclude_each_other_before_source_and_cursor_reads",
                "child_a",
                &root,
                &seam,
                "before-source",
            );
            wait_for_file(&seam.join("before-source.ready"));

            let note_path = root.join("chronicle/20260717/default/100000_300/talents/audio.md");
            fs::create_dir_all(note_path.parent().unwrap()).unwrap();
            fs::write(&note_path, "# Audio").unwrap();
            let blocked_seam = seam.join("blocked-source");
            fs::create_dir_all(&blocked_seam).unwrap();
            let mut blocked = run_child_role(
                "writer_admission::tests::full_process_tests::two_writers_exclude_each_other_before_source_and_cursor_reads",
                "blocked_source",
                &root,
                &blocked_seam,
                "observe",
            );
            assert!(
                blocked.wait().unwrap().success(),
                "blocked source caller must report busy"
            );
            assert!(
                !blocked_seam.join("before-source.observed").exists(),
                "waiting writer must not reach source eligibility or bytes"
            );
            fs::write(seam.join("before-source.release"), b"go").unwrap();
            assert!(
                child_a.wait().unwrap().success(),
                "rescan child must succeed"
            );

            // Test B: Child B in apply_classification_batch pauses at before-cursor
            fs::remove_file(seam.join("before-source.ready")).unwrap();
            fs::remove_file(seam.join("before-source.release")).unwrap();
            let _conn = open_index(&root).unwrap();
            drop(_conn);

            let mut child_b = run_child_role(
                "writer_admission::tests::full_process_tests::two_writers_exclude_each_other_before_source_and_cursor_reads",
                "child_b",
                &root,
                &seam,
                "before-cursor",
            );
            wait_for_file(&seam.join("before-cursor.ready"));

            let blocked_seam = seam.join("blocked-cursor");
            fs::create_dir_all(&blocked_seam).unwrap();
            let mut blocked = run_child_role(
                "writer_admission::tests::full_process_tests::two_writers_exclude_each_other_before_source_and_cursor_reads",
                "blocked_cursor",
                &root,
                &blocked_seam,
                "observe",
            );
            assert!(
                blocked.wait().unwrap().success(),
                "blocked cursor caller must report busy"
            );
            assert!(
                !blocked_seam.join("before-cursor.observed").exists(),
                "waiting writer must not read cursor candidates"
            );
            fs::write(seam.join("before-cursor.release"), b"go").unwrap();
            assert!(
                child_b.wait().unwrap().success(),
                "classification child must succeed"
            );

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
                "writer_admission::tests::full_process_tests::process_death_releases_the_lease_and_reacquire_keeps_the_sidecar",
                "child_hold",
                &root,
                &seam,
                "hold",
            );
            wait_for_file(&seam.join("hold.ready"));
            let sidecar = root.join("indexer/journal.sqlite.lock");
            assert!(sidecar.exists(), "sidecar must exist while lease is held");

            child.kill().expect("kill child holding lease");
            assert!(!child.wait().unwrap().success(), "holder must be killed");

            let prune_res = prune_by_paths(&root, &["dummy"]);
            assert!(
                prune_res.is_ok(),
                "prune after killed child must succeed, got {prune_res:?}"
            );
            assert!(sidecar.exists(), "sidecar must survive reacquisition");

            let _ = fs::remove_dir_all(&root);
        }

        #[test]
        fn all_mutators_wait_for_admission_while_readers_remain_available() {
            if std::env::var("SOLSTONE_TEST_CHILD_ROLE").as_deref() == Ok("holder") {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                reset_index(&journal).unwrap();
                return;
            }
            let root = temp_root("all-mutators");
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();
            let conn = open_index(&root).unwrap();
            conn.execute(
                "INSERT INTO chunks(content, path) VALUES ('writer search needle', 'example.md')",
                [],
            )
            .unwrap();
            drop(conn);
            let before = fs::read(crate::db::db_path(&root)).unwrap();
            let mut holder = run_child_role(
                "writer_admission::tests::full_process_tests::all_mutators_wait_for_admission_while_readers_remain_available",
                "holder",
                &root,
                &seam,
                "hold",
            );
            wait_for_file(&seam.join("hold.ready"));

            let reader = open_index_reader(&root).unwrap();
            let hits: i64 = reader
                .query_row(
                    "SELECT COUNT(*) FROM chunks WHERE chunks MATCH 'needle'",
                    [],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(hits, 1);
            drop(reader);
            inspect_classifications(&root).unwrap();
            inspect_path_lookup(&root).unwrap();
            fingerprint_edge_rows(&root).unwrap();
            assert_eq!(
                crate::migrations::index_stream::migrate_index_stream(&root, true).unwrap(),
                crate::migrations::index_stream::IndexStreamMigration::Current
            );
            assert_eq!(
                fs::read(crate::db::db_path(&root)).unwrap(),
                before,
                "readers must not change schema or stored data"
            );

            type Mutation = fn(&Path) -> Result<(), StoreError>;
            let mutations: [(&str, Mutation); 14] = [
                ("scan", |j| crate::scan::scan_journal(j, false).map(|_| ())),
                ("rescan-file", |j| {
                    rescan_file(
                        j,
                        &j.join("chronicle/20260717/default/100000_300/talents/audio.md"),
                    )
                    .map(|_| ())
                }),
                ("reset", reset_index),
                ("prune-paths", |j| {
                    prune_by_paths(j, &["example.md"]).map(|_| ())
                }),
                ("prune-authored-chat", |j| {
                    prune_authored_chat_paths(j).map(|_| ())
                }),
                ("prune-stream", |j| {
                    prune_chunks_by_stream(j, "default").map(|_| ())
                }),
                ("rebuild-edges", |j| {
                    crate::scan::rebuild_edges(j).map(|_| ())
                }),
                ("fold-entity-edges", |j| {
                    crate::merge::fold_entity_edges_for_recorded_merge(j, "source", "target")
                        .map(|_| ())
                }),
                ("rebuild-edges-fingerprint", |j| {
                    crate::merge::rebuild_edges_for_recorded_merge_undo(j).map(|_| ())
                }),
                ("path-lookup", |j| {
                    crate::chunk_sources::apply_path_lookup(j).map(|_| ())
                }),
                ("classification-batch", |j| {
                    apply_classification_batch(j).map(|_| ())
                }),
                ("reconcile-classifications", |j| {
                    crate::reconcile::reconcile_stale_classifications(j, &mut || {
                        panic!("blocked reconcile must not read a snapshot")
                    })
                    .map(|_| ())
                }),
                ("migrate-index-stream", |j| {
                    crate::migrations::index_stream::migrate_index_stream(j, false).map(|_| ())
                }),
                ("remove-legacy-index", |j| {
                    remove_legacy_index_artifacts(j).map(|_| ())
                }),
            ];
            let threads = mutations.into_iter().map(|(expected, mutate)| {
                let journal = root.clone();
                std::thread::spawn(move || {
                    let result = mutate(&journal);
                    assert!(matches!(result, Err(StoreError::WriterBusy { operation, .. }) if operation == expected), "{expected} must report admission contention: {result:?}");
                })
            }).collect::<Vec<_>>();
            for thread in threads {
                thread.join().unwrap();
            }
            holder.kill().unwrap();
            assert!(!holder.wait().unwrap().success());
            assert!(root.join("indexer/journal.sqlite.lock").exists());
            fs::remove_dir_all(root).unwrap();
        }

        #[test]
        fn memory_rescan_paused_before_database_creation_makes_prune_wait() {
            let role = std::env::var("SOLSTONE_TEST_CHILD_ROLE").unwrap_or_default();
            if role == "child_memory" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let note = journal.join("chronicle").join(memory_note_relative());
                assert!(matches!(
                    rescan_file(&journal, &note).unwrap(),
                    crate::scan::RescanFileStatus::Indexed { .. }
                ));
                return;
            }

            let root = std::env::temp_dir()
                .join(format!("solstone-test-mem-prune-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            let (note_path, note_rel) = seed_memory_original(&root);

            let mut child = run_child_role(
                "writer_admission::tests::full_process_tests::memory_rescan_paused_before_database_creation_makes_prune_wait",
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

            fs::remove_file(&note_path).expect("remove source before pruning");

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
            let (note_path, note_rel) = seed_memory_original(&root);

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
        fn classification_drain_stops_on_a_busy_next_batch() {
            if std::env::var("SOLSTONE_TEST_CHILD_ROLE").as_deref() == Ok("busy_drain") {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let error =
                    crate::classification_batch::drain_classifications(&journal).unwrap_err();
                assert!(matches!(
                    error,
                    StoreError::WriterBusy {
                        operation: "classification-batch",
                        ..
                    }
                ));
                return;
            }
            let root = temp_root("busy-drain");
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();
            seed_classification_batches(&root);
            let mut child = run_child_role(
                "writer_admission::tests::full_process_tests::classification_drain_stops_on_a_busy_next_batch",
                "busy_drain",
                &root,
                &seam,
                "after-classification-release",
            );
            wait_for_file(&seam.join("after-classification-release.ready"));
            let held = IndexAdmission::acquire(&root, "test-competing-update").unwrap();
            fs::write(seam.join("after-classification-release.release"), b"go").unwrap();
            assert!(
                child.wait().unwrap().success(),
                "busy drain must return an explicit error"
            );
            drop(held);
            let status = inspect_classifications(&root).unwrap();
            assert_eq!(status.cursor, "item-31.md");
            assert_eq!(status.remaining, 8);
            assert!(!status.completed);
            fs::remove_dir_all(root).unwrap();
        }

        fn seed_classification_batches(root: &Path) {
            let conn = open_index(root).unwrap();
            for n in 0..40 {
                conn.execute(
                    "INSERT INTO chunks(content, path) VALUES ('text', ?1)",
                    [format!("item-{n:02}.md")],
                )
                .unwrap();
            }
            drop(conn);
            crate::chunk_sources::apply_path_lookup(root).unwrap();
        }

        #[test]
        fn classification_batch_releases_admission_before_the_post_commit_barrier() {
            let role = std::env::var("SOLSTONE_TEST_CHILD_ROLE").unwrap_or_default();
            if role == "child_cls_barrier" {
                let journal = PathBuf::from(std::env::var("SOLSTONE_TEST_JOURNAL").unwrap());
                let status = crate::classification_batch::drain_classifications(&journal).unwrap();
                let seam = PathBuf::from(std::env::var("SOLSTONE_TEST_INDEX_WRITER_SEAM").unwrap());
                fs::write(
                    seam.join("drained.json"),
                    status.to_json_value().to_string(),
                )
                .unwrap();
                return;
            }

            let root =
                std::env::temp_dir().join(format!("solstone-test-cls-rel-{}", std::process::id()));
            let seam = root.join("seam");
            fs::create_dir_all(&seam).unwrap();

            seed_classification_batches(&root);

            let mut child = run_child_role(
                "writer_admission::tests::full_process_tests::classification_batch_releases_admission_before_the_post_commit_barrier",
                "child_cls_barrier",
                &root,
                &seam,
                "after-classification-release",
            );
            wait_for_file(&seam.join("after-classification-release.ready"));

            let reader = open_index_reader(&root).unwrap();
            let count: i64 = reader
                .query_row("SELECT COUNT(*) FROM chunk_classification", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 32, "first batch must be finite and committed");
            drop(reader);
            let prune_res = prune_by_paths(&root, &["item-39.md"]);
            assert!(
                prune_res.is_ok(),
                "parent must be able to acquire writer lease while child is at post-commit barrier, got {prune_res:?}"
            );

            fs::write(seam.join("after-classification-release.release"), b"go").unwrap();
            assert!(
                child.wait().unwrap().success(),
                "classification child must succeed"
            );
            let status: serde_json::Value =
                serde_json::from_slice(&fs::read(seam.join("drained.json")).unwrap()).unwrap();
            assert_eq!(status["completed"], true);
            assert_eq!(
                status["cursor"], "item-38.md",
                "next admission must reread candidates after pruning"
            );
            assert_eq!(status["remaining"], 0);
            assert_eq!(status["missing"], 0);
            let reader = open_index_reader(&root).unwrap();
            let count: i64 = reader
                .query_row("SELECT COUNT(*) FROM chunk_classification", [], |r| {
                    r.get(0)
                })
                .unwrap();
            assert_eq!(count, 39);
            drop(reader);

            let _ = fs::remove_dir_all(&root);
        }
    }
}

#[cfg(test)]
mod error_tests {
    use super::*;

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
}
