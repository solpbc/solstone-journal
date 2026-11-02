// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#![allow(clippy::disallowed_methods, clippy::disallowed_types)]

use super::*;
use crate::db::{db_path, open_index};
use crate::scan::{scan_journal, with_reconcile_stat_error};
use solstone_core_journal_io::locking::hold_lock;
use std::path::PathBuf;

struct Journal(PathBuf);
impl Journal {
    fn new() -> Self {
        let root = crate::test_support::reserve_temp_path("index-todo");
        fs::create_dir_all(root.join("config")).unwrap();
        fs::write(
            root.join("config/journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
        Self(root)
    }
    fn path(&self) -> &Path {
        &self.0
    }
    fn save(&self, rel: &str, text: &str) -> PathBuf {
        let path = solstone_core_format::paths::resolve_journal_path(self.path(), rel).unwrap();
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, text).unwrap();
        path
    }
}
impl Drop for Journal {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

const NOTE: &str = "20260101/desktop/120000_300/talents/activity.md";

#[test]
fn screen_text_path_hints_build_timed_pins_and_remove_all_derived_rows() {
    let journal = Journal::new();
    let rel = "20260101/desktop/120000_300/talents/screen_text.md";
    let source = journal.save(
        rel,
        "# screen text\n\n## 00:12 · terminal\n- queued literal\n",
    );
    let lease = hold_lock(db_path(journal.path()), Default::default()).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    assert!(!db_path(journal.path()).exists());
    assert_eq!(inspect(journal.path()).unwrap().keys, [rel]);
    drop(lease);

    let report = drain_now(journal.path()).unwrap();
    assert_eq!((report.processed, report.failed), (1, 0));
    let conn = open_index(journal.path()).unwrap();
    let category: String = conn
        .query_row(
            "SELECT category FROM chunk_classification WHERE path=?",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(category, "screen_text");
    let offset_ms: i64 = conn
        .query_row(
            "SELECT offset_ms FROM chunk_group_offset WHERE path=?",
            [rel],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(offset_ms, 12_000);
    assert!(crate::screen_text_source_current(&conn, journal.path(), rel).unwrap());
    drop(conn);

    fs::remove_file(&source).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    let conn = open_index(journal.path()).unwrap();
    for table in [
        "files",
        "chunks",
        "chunk_classification",
        "chunk_group_offset",
        "screen_text_source",
    ] {
        let remaining: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE path=?"),
                [rel],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(remaining, 0, "removal retained rows in {table}");
    }
}

#[test]
fn observation_save_and_removal_queue_without_the_index_writer() {
    let journal = Journal::new();
    journal.save("facets/work/entities/alice/entity.json", "{}");
    let rel = "facets/work/entities/alice/observations.jsonl";
    let lease = hold_lock(db_path(journal.path()), Default::default()).unwrap();
    solstone_core_entity::apply_observation_change(
        journal.path(),
        "work",
        "alice",
        solstone_core_entity::ObservationChange::Append {
            content: "kestrel observation".into(),
            source_day: None,
            relation: None,
            actor: "fixture",
        },
    )
    .unwrap();
    assert_eq!(
        inspect(journal.path()).unwrap().keys,
        ["@entity-search", rel]
    );
    assert!(!db_path(journal.path()).exists());
    drop(lease);
    assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    // The entity change queued its connections refresh for the next drain.
    assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    assert!(
        chunks(journal.path(), rel)
            .iter()
            .any(|text| text.contains("kestrel"))
    );
    let check = scan_journal(journal.path(), false).unwrap();
    assert_eq!((check.indexed, check.removed), (0, 0));
    let lease = hold_lock(db_path(journal.path()), Default::default()).unwrap();
    solstone_core_entity::apply_observation_change(
        journal.path(),
        "work",
        "alice",
        solstone_core_entity::ObservationChange::RemoveFile,
    )
    .unwrap();
    assert_eq!(
        inspect(journal.path()).unwrap().keys,
        ["@entity-search", rel]
    );
    drop(lease);
    assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    assert!(chunks(journal.path(), rel).is_empty());
    let check = scan_journal(journal.path(), false).unwrap();
    assert_eq!((check.indexed, check.removed), (0, 0));
}

fn listed(journal: &Path) -> Vec<Entry> {
    entry_paths(&root(journal))
        .unwrap()
        .iter()
        .filter_map(|p| read_entry(p, false).unwrap())
        .collect()
}

fn chunks(journal: &Path, path: &str) -> Vec<String> {
    let conn = open_index(journal).unwrap();
    conn.prepare("SELECT content FROM chunks WHERE path=? ORDER BY content")
        .unwrap()
        .query_map([path], |row| row.get(0))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap()
}

#[test]
fn enqueue_does_not_wait_on_sqlite_and_resaves_preserve_retry_state() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "a kestrel meeting");
    let lease = hold_lock(db_path(journal.path()), LockOptions::default()).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    assert!(
        !db_path(journal.path()).exists(),
        "enqueue must not open SQLite"
    );
    let mut entry = listed(journal.path()).pop().unwrap();
    assert_eq!(entry.key, NOTE);
    entry.attempts = 2;
    entry.not_before_ms = now_ms() + RETRY_MS;
    rewrite(&root(journal.path()).join(entry.filename()), &entry).unwrap();
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    assert_eq!(listed(journal.path()), vec![entry]);
    drop(lease);
    let report = drain_now(journal.path()).unwrap();
    assert_eq!(report.processed, 1);
    assert_eq!(report.failed, 0);
    assert!(!chunks(journal.path(), NOTE).is_empty());
    let scan = scan_journal(journal.path(), false).unwrap();
    assert_eq!((scan.indexed, scan.removed), (0, 0));
}

#[test]
fn a_corrupt_existing_hint_cannot_report_enqueue_success() {
    let journal = Journal::new();
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    let entry = listed(journal.path()).pop().unwrap();
    let path = root(journal.path()).join(entry.filename());
    fs::write(&path, b"{").unwrap();
    assert_eq!(
        enqueue_path(journal.path(), Path::new(NOTE))
            .unwrap_err()
            .kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(fs::read(path).unwrap(), b"{");
}

#[test]
fn removal_reports_a_failed_enqueue_after_the_content_is_gone() {
    let journal = Journal::new();
    journal.save(NOTE, "owner content");
    let target = solstone_core_retention::Target {
        day: "20260101".into(),
        stream: "desktop".into(),
        dir: "120000_300".into(),
    };
    let outcome = solstone_core_retention::door::remove_segments(
        journal.path(),
        &[target],
        "2026-01-02T00:00:00Z",
        solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
        "fixture",
    );
    assert!(
        !outcome.removed_paths().collect::<Vec<_>>().is_empty(),
        "{outcome:?}"
    );
    assert!(!journal.path().join("chronicle").join(NOTE).exists());
    fs::create_dir_all(journal.path().join("indexer")).unwrap();
    fs::write(journal.path().join("indexer/todo"), "blocked directory").unwrap();
    let error = solstone_core_retention::door::notify_index(
        &crate::retention::RetentionIndex::new(journal.path()),
        &outcome,
    )
    .unwrap_err();
    assert!(error.reason.contains("could not be queued"));
}

#[test]
fn deletion_prunes_every_family_both_path_shapes_and_only_the_directory_prefix() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "removed kestrel");
    let sibling = NOTE.replace("120000_300/", "120000_3000/");
    let sibling_file = journal.save(&sibling, "retained osprey");
    enqueue_path(journal.path(), &source).unwrap();
    enqueue_path(journal.path(), &sibling_file).unwrap();
    assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    {
        let conn = open_index(journal.path()).unwrap();
        for path in [NOTE.to_owned(), format!("chronicle/{NOTE}")] {
            conn.execute(
                "INSERT INTO files(path,mtime) VALUES (?1,0) ON CONFLICT(path) DO NOTHING",
                [&path],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks(content,path) VALUES ('old alternate shape',?1)",
                [&path],
            )
            .unwrap();
            conn.execute("INSERT INTO edge_files(path,mtime) VALUES (?1,0)", [&path])
                .unwrap();
            conn.execute("INSERT INTO edges(src,dst,kind,directed,source,path,weight) VALUES ('a','b','cooccur',0,'fixture',?1,1)", [&path]).unwrap();
        }
    }
    let lease = hold_lock(db_path(journal.path()), Default::default()).unwrap();
    let removed = solstone_core_retention::door::remove_segments(
        journal.path(),
        &[solstone_core_retention::Target {
            day: "20260101".into(),
            stream: "desktop".into(),
            dir: "120000_300".into(),
        }],
        "2026-01-02T00:00:00Z",
        solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
        "fixture",
    );
    solstone_core_retention::door::notify_index(
        &crate::retention::RetentionIndex::new(journal.path()),
        &removed,
    )
    .unwrap();
    let mut expected = removed
        .removed_paths()
        .map(|path| {
            path.as_str()
                .strip_prefix("chronicle/")
                .unwrap_or(path.as_str())
                .to_owned()
        })
        .collect::<Vec<_>>();
    expected.sort();
    expected.dedup();
    assert!(!expected.is_empty());
    assert_eq!(inspect(journal.path()).unwrap().keys, expected);
    drop(lease);
    assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    let conn = open_index(journal.path()).unwrap();
    for table in [
        "chunks",
        "files",
        "memory_originals",
        "edges",
        "edge_files",
        "chunk_classification",
        "chunk_sources",
    ] {
        let count: i64 = conn
            .query_row(
                &format!("SELECT count(*) FROM {table} WHERE path=?1 OR path=?2"),
                [NOTE, &format!("chronicle/{NOTE}")],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(count, 0, "stale {table} rows");
    }
    assert!(!chunks(journal.path(), &sibling).is_empty());
    let check = scan_journal(journal.path(), false).unwrap();
    assert_eq!((check.indexed, check.removed), (0, 0));
}

#[test]
fn a_recreated_path_is_indexed_and_a_present_directory_drops_only_missing_children() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "old kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    drain_now(journal.path()).unwrap();
    fs::remove_file(&source).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    fs::write(&source, "new osprey").unwrap();
    drain_now(journal.path()).unwrap();
    assert!(
        chunks(journal.path(), NOTE)
            .iter()
            .any(|text| text.contains("new osprey"))
    );
    fs::remove_file(&source).unwrap();
    let other = NOTE.replace("activity.md", "brief.md");
    journal.save(&other, "heron notes");
    enqueue_path(journal.path(), Path::new("20260101/desktop/120000_300")).unwrap();
    drain_now(journal.path()).unwrap();
    assert!(chunks(journal.path(), NOTE).is_empty());
    assert!(!chunks(journal.path(), &other).is_empty());
}

#[test]
fn non_notfound_stat_errors_keep_rows_then_drop_as_a_present_file_after_three_tries() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "kestrel retained");
    enqueue_path(journal.path(), &source).unwrap();
    drain_now(journal.path()).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    for attempt in 1..=3 {
        let report = with_reconcile_stat_error(&source, io::ErrorKind::PermissionDenied, || {
            drain_now(journal.path()).unwrap()
        });
        assert_eq!(report.failed, 1);
        assert!(!chunks(journal.path(), NOTE).is_empty());
        if attempt < 3 {
            assert_eq!(listed(journal.path())[0].attempts, attempt);
        }
    }
    assert!(listed(journal.path()).is_empty());
    assert_eq!(inspect(journal.path()).unwrap().failed_deletions, 0);
    let logs = journal
        .path()
        .join("chronicle")
        .join(chrono::Utc::now().format("%Y%m%d").to_string())
        .join("health/indexer.jsonl");
    let events = fs::read_to_string(logs).unwrap();
    assert!(events.contains("injected source stat error"));
    assert_eq!(
        events
            .lines()
            .filter(|line| line.contains("\"outcome\":\"failed\""))
            .count(),
        3
    );
}

#[test]
fn failed_invalidation_keeps_the_last_attempt_unclaimed() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "cached kestrel");
    scan_journal(journal.path(), false).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    let mut entry = listed(journal.path()).pop().unwrap();
    entry.attempts = 2;
    rewrite(&root(journal.path()).join(entry.filename()), &entry).unwrap();
    let conn = open_index(journal.path()).unwrap();
    conn.execute_batch("CREATE TRIGGER refuse_invalidation BEFORE UPDATE OF mtime ON files BEGIN SELECT RAISE(ABORT, 'fixture metadata failure'); END").unwrap();
    assert!(drain_now(journal.path()).is_err());
    assert_eq!(listed(journal.path()), [entry]);
    assert_eq!(inspect(journal.path()).unwrap().claimed, 0);
    assert!(!chunks(journal.path(), NOTE).is_empty());
    conn.execute_batch("DROP TRIGGER refuse_invalidation")
        .unwrap();
    assert_eq!(drain_now(journal.path()).unwrap().processed, 1);
    assert!(listed(journal.path()).is_empty());
}

#[test]
fn failed_deletions_survive_three_tries_and_an_empty_day_catch_all() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "remove this kestrel");
    enqueue_path(journal.path(), &source).unwrap();
    drain_now(journal.path()).unwrap();
    fs::remove_dir_all(journal.path().join("chronicle/20260101")).unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    open_index(journal.path()).unwrap().execute_batch("CREATE TRIGGER refuse_delete BEFORE DELETE ON files BEGIN SELECT RAISE(ABORT,'fixture deletion refusal'); END;").unwrap();
    for _ in 0..3 {
        assert_eq!(drain_now(journal.path()).unwrap().failed, 1);
    }
    assert_eq!(inspect(journal.path()).unwrap().failed_deletions, 1);
    let entry = listed(journal.path()).pop().unwrap();
    assert!(entry.not_before_ms > now_ms() + RETRY_MS);
    scan_journal(journal.path(), false).unwrap();
    assert_eq!(listed(journal.path()), vec![entry]);
    open_index(journal.path())
        .unwrap()
        .execute_batch("DROP TRIGGER refuse_delete;")
        .unwrap();
    assert_eq!(drain_now(journal.path()).unwrap().processed, 1);
    assert_eq!(inspect(journal.path()).unwrap().failed_deletions, 0);
    assert!(chunks(journal.path(), NOTE).is_empty());
}

#[test]
fn a_fresh_entry_keeps_due_now_and_inherits_failed_or_crashed_claim_attempts() {
    let journal = Journal::new();
    let source = journal.save(NOTE, "present kestrel");
    let claims = root(journal.path()).join("claimed");
    fs::create_dir_all(&claims).unwrap();
    for attempt in 1..=3 {
        enqueue_path(journal.path(), &source).unwrap();
        let entry = listed(journal.path()).pop().unwrap();
        assert_eq!(entry.attempts, attempt - 1);
        let path = root(journal.path()).join(entry.filename());
        let claim = claims.join(entry.filename());
        fs::rename(path, &claim).unwrap();
        enqueue_path(journal.path(), &source).unwrap();
        return_failed(journal.path(), entry, &claim, "fixture crash").unwrap();
        if attempt < 3 {
            let entry = listed(journal.path()).pop().unwrap();
            assert_eq!((entry.attempts, entry.not_before_ms), (attempt, 0));
        }
    }
    assert!(
        listed(journal.path()).is_empty(),
        "re-saves must not make a crash loop immortal"
    );
}

#[test]
fn recovery_carries_the_counter_from_a_partly_returned_failure() {
    let journal = Journal::new();
    journal.save(NOTE, "saved kestrel");
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    let original = listed(journal.path()).pop().unwrap();
    let claim = root(journal.path())
        .join("claimed")
        .join(original.filename());
    fs::create_dir_all(claim.parent().unwrap()).unwrap();
    publish_if_absent(&claim, &original).unwrap();
    let destination = root(journal.path()).join(original.filename());
    let mut returned = original;
    returned.attempts = 1;
    rewrite(&destination, &returned).unwrap();
    let _lock = hold_lock(journal.path().join("indexer/drain"), Default::default()).unwrap();
    let recovered = read_entry(&claim, false).unwrap().unwrap();
    return_failed(
        journal.path(),
        recovered,
        &claim,
        "interrupted failure return",
    )
    .unwrap();
    assert_eq!(listed(journal.path())[0].attempts, 2);
}

#[test]
fn unknown_kinds_fields_temps_and_set_asides_are_untouched_and_never_due() {
    let journal = Journal::new();
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    let entry = listed(journal.path()).pop().unwrap();
    let path = root(journal.path()).join(entry.filename());
    for extra_field in [false, true] {
        let mut value = serde_json::to_value(&entry).unwrap();
        if extra_field {
            value["new_version_field"] = serde_json::json!(true);
        } else {
            value["kind"] = serde_json::json!("new-kind");
        }
        let bytes = serde_json::to_vec(&value).unwrap();
        fs::write(&path, &bytes).unwrap();
        fs::write(root(journal.path()).join(".partial.json"), b"{").unwrap();
        fs::write(root(journal.path()).join("old.wedged-123.json"), b"{}").unwrap();
        fs::create_dir_all(root(journal.path()).join("claimed")).unwrap();
        enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
        assert!(!has_due(journal.path(), u64::MAX).unwrap());
        assert_eq!(drain_now(journal.path()).unwrap().processed, 0);
        assert_eq!(fs::read(&path).unwrap(), bytes);
    }
    // Re-upgrading to the version that recognizes the entry sees its original attempts.
    rewrite(&path, &entry).unwrap();
    journal.save(NOTE, "upgraded kestrel");
    assert_eq!(drain_now(journal.path()).unwrap().processed, 1);
}

#[test]
fn a_busy_admission_is_bounded_and_does_not_spend_an_attempt() {
    let journal = Journal::new();
    journal.save(NOTE, "kestrel");
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    let lease = hold_lock(db_path(journal.path()), LockOptions::default()).unwrap();
    let started = Instant::now();
    let report = drain(
        journal.path(),
        DrainOptions {
            force: true,
            budget: Duration::from_millis(100),
        },
    )
    .unwrap();
    assert!(report.timed_out);
    assert!(started.elapsed() < Duration::from_secs(2));
    assert_eq!(listed(journal.path())[0].attempts, 0);
    drop(lease);
    assert_eq!(drain_now(journal.path()).unwrap().processed, 1);
}

#[test]
fn a_second_drain_cannot_recover_an_active_claim() {
    let journal = Journal::new();
    journal.save(NOTE, "kestrel");
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    let entry = listed(journal.path()).pop().unwrap();
    let lease = hold_lock(journal.path().join("indexer/drain"), LockOptions::default()).unwrap();
    fs::create_dir_all(root(journal.path()).join("claimed")).unwrap();
    let claim = root(journal.path()).join("claimed").join(entry.filename());
    fs::rename(root(journal.path()).join(entry.filename()), &claim).unwrap();
    let other = drain(
        journal.path(),
        DrainOptions {
            force: true,
            budget: Duration::from_millis(100),
        },
    )
    .unwrap();
    assert!(other.timed_out);
    assert_eq!(other.recovered, 0);
    assert_eq!(read_entry(&claim, false).unwrap().unwrap().attempts, 0);
    drop(lease);
    let recovered = drain_now(journal.path()).unwrap();
    assert_eq!((recovered.recovered, recovered.processed), (1, 1));
}

#[test]
fn path_only_incremental_queries_equal_a_full_rebuild() {
    let journal = Journal::new();
    drop(open_index(journal.path()).unwrap());
    let empty_edges = crate::merge::fingerprint_edge_rows(journal.path()).unwrap();
    let lease = hold_lock(db_path(journal.path()), Default::default()).unwrap();
    for (entity, name) in [("alice", "Alice Edge"), ("bob", "Bob Edge")] {
        solstone_core_entity::save_entity_identity(
            journal.path(),
            entity,
            &serde_json::json!({"id":entity,"name":name,"type":"Person"}),
            None,
        )
        .unwrap();
        journal.save(&format!("facets/work/entities/{entity}/entity.json"), "{}");
    }
    assert_eq!(inspect(journal.path()).unwrap().keys, ["@entity-search"]);
    drop(lease);
    let edge = journal.save("facets/work/entities/20260101.jsonl", "{\"name\":\"Alice Edge\",\"segments\":[\"s1\"]}\n{\"name\":\"Bob Edge\",\"segments\":[\"s1\"]}\n");
    enqueue_path(journal.path(), &edge).unwrap();
    let source = journal.save(NOTE, "kestrel planning");
    enqueue_path(journal.path(), &source).unwrap();
    drain_now(journal.path()).unwrap();
    journal.save(NOTE, "osprey planning");
    enqueue_path(journal.path(), &source).unwrap();
    let other = NOTE.replace("activity.md", "brief.md");
    let other_path = journal.save(&other, "heron notes");
    enqueue_path(journal.path(), &other_path).unwrap();
    drain_now(journal.path()).unwrap();
    fs::remove_file(&other_path).unwrap();
    enqueue_path(journal.path(), &other_path).unwrap();
    journal.save(&other, "wren notes");
    drain_now(journal.path()).unwrap();
    let contents = |journal: &Path| {
        let conn = open_index(journal).unwrap();
        conn.prepare("SELECT path,content FROM chunks ORDER BY path,content")
            .unwrap()
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .unwrap()
            .collect::<Result<Vec<_>, _>>()
            .unwrap()
    };
    let incremental = contents(journal.path());
    assert!(!incremental.is_empty());
    assert!(
        incremental
            .iter()
            .any(|(path, _)| path.starts_with("entity_search:"))
    );
    let edges = crate::merge::fingerprint_edge_rows(journal.path()).unwrap();
    assert_ne!(edges, empty_edges);
    crate::db::reset_index(journal.path()).unwrap();
    scan_journal(journal.path(), true).unwrap();
    assert_eq!(incremental, contents(journal.path()));
    assert_eq!(
        edges,
        crate::merge::fingerprint_edge_rows(journal.path()).unwrap()
    );
}

#[test]
fn a_finished_light_receipt_survives_a_failed_later_pass() {
    let journal = Journal::new();
    journal.save(NOTE, "kestrel");
    scan_journal(journal.path(), false).unwrap();
    let conn = open_index(journal.path()).unwrap();
    let finished = crate::generation::read_last_finished_check(&conn)
        .unwrap()
        .unwrap();
    crate::generation::record_scan(
        &conn,
        &crate::generation::ScanReceipt {
            finished_at_ms: finished.finished_at_ms + 1,
            error: Some("fixture crash".into()),
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(
        crate::generation::read_last_finished_check(&conn).unwrap(),
        Some(finished)
    );
}

struct DrainChild(std::process::Child);
impl Drop for DrainChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn child_drain(test: &str, journal: &Path, seam: &Path, pause: &str) -> DrainChild {
    DrainChild(
        std::process::Command::new(std::env::current_exe().unwrap())
            .args(["--exact", test, "--nocapture"])
            .env("SOLSTONE_TEST_TODO_CHILD", journal)
            .env("SOLSTONE_TEST_INDEX_WRITER_SEAM", seam)
            .env("SOLSTONE_TEST_INDEX_WRITER_PAUSE", pause)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .unwrap(),
    )
}

fn wait_for(path: &Path) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        assert!(
            Instant::now() < deadline,
            "missing witness {}",
            path.display()
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn run_child_drain() -> bool {
    let Some(journal) = std::env::var_os("SOLSTONE_TEST_TODO_CHILD") else {
        return false;
    };
    let journal = PathBuf::from(journal);
    let report = drain_now(&journal).unwrap();
    fs::write(
        journal.join("drain-report.json"),
        serde_json::to_vec(&report).unwrap(),
    )
    .unwrap();
    true
}

#[test]
fn a_killed_claim_is_recovered_and_counted_before_it_is_processed() {
    if run_child_drain() {
        return;
    }
    let journal = Journal::new();
    journal.save(NOTE, "kestrel after a crash");
    enqueue_path(journal.path(), Path::new(NOTE)).unwrap();
    let entry = listed(journal.path()).pop().unwrap();
    let seam = journal.path().join("seam");
    fs::create_dir_all(&seam).unwrap();
    let mut child = child_drain(
        "todo::tests::a_killed_claim_is_recovered_and_counted_before_it_is_processed",
        journal.path(),
        &seam,
        "todo-claimed",
    );
    wait_for(&seam.join("todo-claimed.ready"));
    let claim = root(journal.path()).join("claimed").join(entry.filename());
    assert_eq!(read_entry(&claim, false).unwrap().unwrap().attempts, 0);
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    let report = drain_now(journal.path()).unwrap();
    assert_eq!((report.recovered, report.processed), (1, 1));
    assert!(!claim.exists());
    assert!(!chunks(journal.path(), NOTE).is_empty());
    let logs = journal
        .path()
        .join("chronicle")
        .join(chrono::Utc::now().format("%Y%m%d").to_string())
        .join("health/indexer.jsonl");
    assert!(
        fs::read_to_string(logs)
            .unwrap()
            .contains("previous index task stopped")
    );
}

#[test]
fn three_killed_claims_with_resaves_leave_same_second_content_for_the_catch_all() {
    if run_child_drain() {
        return;
    }
    let journal = Journal::new();
    let source = journal.save(NOTE, "previous kestrel");
    scan_journal(journal.path(), false).unwrap();
    let modified = fs::metadata(&source).unwrap().modified().unwrap();
    fs::write(&source, "latest kestrel").unwrap();
    // Setting a time needs a writable handle on Windows.
    fs::OpenOptions::new()
        .write(true)
        .open(&source)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    enqueue_path(journal.path(), &source).unwrap();
    let entry = listed(journal.path()).pop().unwrap();
    let claim = root(journal.path()).join("claimed").join(entry.filename());
    let seam = journal.path().join("seam");
    fs::create_dir_all(&seam).unwrap();
    for attempt in 0..3 {
        let ready = seam.join("todo-claimed.ready");
        if ready.exists() {
            fs::remove_file(&ready).unwrap();
        }
        let mut child = child_drain(
            "todo::tests::three_killed_claims_with_resaves_leave_same_second_content_for_the_catch_all",
            journal.path(),
            &seam,
            "todo-claimed",
        );
        wait_for(&ready);
        assert_eq!(
            read_entry(&claim, false).unwrap().unwrap().attempts,
            attempt
        );
        enqueue_path(journal.path(), &source).unwrap();
        child.0.kill().unwrap();
        assert!(!child.0.wait().unwrap().success());
    }
    let recovered = drain_now(journal.path()).unwrap();
    assert_eq!((recovered.recovered, recovered.processed), (1, 0));
    assert!(inspect(journal.path()).unwrap().keys.is_empty());
    assert!(
        chunks(journal.path(), NOTE)
            .iter()
            .any(|text| text.contains("previous"))
    );
    let check = scan_journal(journal.path(), false).unwrap();
    assert_eq!((check.indexed, check.failed), (1, 0));
    assert!(
        chunks(journal.path(), NOTE)
            .iter()
            .any(|text| text.contains("latest"))
    );
}

#[test]
fn a_competing_writer_can_acquire_between_drain_batches() {
    if run_child_drain() {
        return;
    }
    let journal = Journal::new();
    // One full batch, which a snapshot may bound, and one entry after it.
    let batch = BATCH_SIZE.min(SNAPSHOT_ENTRIES);
    for n in 0..batch + 1 {
        let path = NOTE.replace("120000_300", &format!("12{n:04}_300"));
        journal.save(&path, "kestrel batch");
        enqueue_path(journal.path(), Path::new(&path)).unwrap();
    }
    let seam = journal.path().join("seam");
    fs::create_dir_all(&seam).unwrap();
    let mut child = child_drain(
        "todo::tests::a_competing_writer_can_acquire_between_drain_batches",
        journal.path(),
        &seam,
        "todo-batch-released",
    );
    wait_for(&seam.join("todo-batch-released.ready"));
    let other = IndexAdmission::acquire(journal.path(), "fixture-between-batches").unwrap();
    assert_eq!(listed(journal.path()).len(), 1);
    fs::write(seam.join("todo-batch-released.release"), b"continue").unwrap();
    drop(other);
    assert!(child.0.wait().unwrap().success());
    let report: serde_json::Value =
        serde_json::from_slice(&fs::read(journal.path().join("drain-report.json")).unwrap())
            .unwrap();
    assert_eq!(report["batches"], 2);
    assert_eq!(report["processed"], batch + 1);
}

mod calendar_delete {
    use super::*;
    use sha2::Digest;

    const GOLDEN: &str = include_str!("../../../fixtures/calendar_notes/golden.jsonl");
    const MEETING: &str = "6CF25C80-68D2-44D9-B52C-6CAD5BEC8C10";

    fn calendar_journal() -> Journal {
        let journal = Journal::new();
        fs::create_dir_all(journal.path().join("streams")).unwrap();
        fs::write(
            journal.path().join("streams/phone_calendar.json"),
            serde_json::json!({
                "name": "phone_calendar", "kind": "observer", "host": null, "platform": null,
                "created_at": 0, "last_day": null, "last_segment": null, "seq": 1,
                "cid": "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "source": "calendar",
            })
            .to_string(),
        )
        .unwrap();
        journal
    }

    /// Store one calendar note file the way ingest does, and queue it.
    fn store(journal: &Journal, day: &str, segment: &str, line: &str) -> String {
        let bytes = format!("{line}\n");
        let name = format!(
            "calendar_{:x}.jsonl",
            sha2::Sha256::digest(bytes.as_bytes())
        );
        let rel = format!("{day}/phone_calendar/{segment}/{name}");
        journal.save(
            &format!("{day}/phone_calendar/{segment}/stream.json"),
            r#"{"stream":"phone_calendar","prev_day":null,"prev_segment":null,"seq":1}"#,
        );
        let source = journal.save(&rel, &bytes);
        enqueue_path(journal.path(), &source).unwrap();
        rel
    }

    fn owner_deletes(journal: &Journal, day: &str, segment: &str) {
        let removed = solstone_core_retention::door::remove_segments(
            journal.path(),
            &[solstone_core_retention::Target {
                day: day.into(),
                stream: "phone_calendar".into(),
                dir: segment.into(),
            }],
            "2026-10-23T00:00:00Z",
            solstone_core_retention::tombstone::RemovalReason::OwnerSegmentDelete,
            "fixture",
        );
        assert!(removed.removed_paths().next().is_some(), "{removed:?}");
        solstone_core_retention::door::notify_index(
            &crate::retention::RetentionIndex::new(journal.path()),
            &removed,
        )
        .unwrap();
        assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
    }

    fn listed(journal: &Journal) -> Vec<(String, String)> {
        let conn = open_index(journal.path()).unwrap();
        let mut statement = conn
            .prepare(
                "SELECT c.day, c.path FROM chunks c JOIN calendar_hits h ON h.rowid=c.rowid \
                 WHERE h.version='latest' AND h.event=? ORDER BY c.day",
            )
            .unwrap();
        statement
            .query_map([MEETING], |row| Ok((row.get(0)?, row.get(1)?)))
            .unwrap()
            .collect::<Result<_, _>>()
            .unwrap()
    }

    fn text_left(journal: &Journal, word: &str) -> i64 {
        let conn = open_index(journal.path()).unwrap();
        let tables = {
            let mut statement = conn
                .prepare("SELECT name FROM sqlite_master WHERE type='table' AND name NOT LIKE 'chunks%' AND name NOT LIKE 'sqlite_%'")
                .unwrap();
            statement
                .query_map([], |row| row.get::<_, String>(0))
                .unwrap()
                .collect::<Result<Vec<_>, _>>()
                .unwrap()
        };
        let mut found = 0;
        for table in tables {
            let mut statement = conn.prepare(&format!("SELECT * FROM {table}")).unwrap();
            let columns = statement.column_count();
            let mut rows = statement.query([]).unwrap();
            while let Some(row) = rows.next().unwrap() {
                for column in 0..columns {
                    if let rusqlite::types::ValueRef::Text(bytes) = row.get_ref(column).unwrap()
                        && String::from_utf8_lossy(bytes).contains(word)
                    {
                        found += 1;
                    }
                }
            }
        }
        found
            + conn
                .query_row(
                    "SELECT count(*) FROM chunks WHERE chunks MATCH ?",
                    [word],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
    }

    #[test]
    fn an_owner_delete_through_the_work_list_falls_back_in_both_orders() {
        let added = GOLDEN.lines().next().unwrap();
        let moved = added
            .replace(r#""change":"added""#, r#""change":"changed""#)
            .replace("2026-10-19T09:42:00-06:00", "2026-10-20T08:01:00-06:00")
            .replace("2026-10-27T10:00:00-06:00", "2026-10-28T14:00:00-06:00")
            .replace("2026-10-27T11:00:00-06:00", "2026-10-28T15:00:00-06:00")
            .replace("design review", "design review in the ocelot room");
        for reverse in [false, true] {
            let journal = calendar_journal();
            let mut stores = vec![
                ("20261019", "094000_300", added.to_owned()),
                ("20261020", "080000_300", moved.clone()),
            ];
            if reverse {
                stores.reverse();
            }
            let mut files = Vec::new();
            for (day, segment, line) in &stores {
                files.push(store(&journal, day, segment, line));
                assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
            }
            if reverse {
                files.reverse();
            }
            // The journal keeps days in UTC here: 14:00 at -06:00 is 20:00.
            assert_eq!(
                listed(&journal),
                [("20261028".to_owned(), files[1].clone())]
            );
            assert!(text_left(&journal, "ocelot") > 0);
            owner_deletes(&journal, "20261020", "080000_300");
            assert_eq!(
                listed(&journal),
                [("20261027".to_owned(), files[0].clone())],
                "reverse {reverse}"
            );
            assert_eq!(text_left(&journal, "ocelot"), 0);
        }
    }

    #[test]
    fn deleting_a_removal_through_the_work_list_brings_the_meeting_back() {
        let journal = calendar_journal();
        let lines = GOLDEN.lines().collect::<Vec<_>>();
        store(&journal, "20261022", "080500_300", lines[4]);
        store(&journal, "20261019", "094000_300", lines[0]);
        assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
        assert!(listed(&journal).is_empty());
        // Deleting a segment that holds only an older note changes nothing.
        let older = lines[0]
            .replace("2026-10-19T09:42:00-06:00", "2026-10-18T10:00:00-06:00")
            .replace("2026-10-27T10:00", "2026-10-26T10:00")
            .replace("2026-10-27T11:00", "2026-10-26T11:00");
        store(&journal, "20261018", "100000_300", &older);
        assert_eq!(drain_now(journal.path()).unwrap().failed, 0);
        owner_deletes(&journal, "20261018", "100000_300");
        assert!(listed(&journal).is_empty());
        owner_deletes(&journal, "20261022", "080500_300");
        assert_eq!(listed(&journal).len(), 1);
        assert_eq!(listed(&journal)[0].0, "20261027");
    }
}

#[test]
fn an_entity_change_brings_the_connections_it_reaches_current() {
    let journal = Journal::new();
    for (id, name) in [("alice", "Alice Edge"), ("bob", "Bob Edge")] {
        journal.save(
            &format!("entities/{id}/entity.json"),
            &format!(r#"{{"name":"{name}","type":"Person"}}"#),
        );
        journal.save(&format!("facets/work/entities/{id}/entity.json"), "{}");
    }
    let rel = "facets/work/entities/20260304.jsonl";
    journal.save(
        rel,
        "{\"name\":\"Ally\",\"segments\":[\"s1\"]}\n{\"name\":\"Bob Edge\",\"segments\":[\"s1\"]}\n",
    );
    scan_journal(journal.path(), true).unwrap();
    let edges = |journal: &Journal| -> i64 {
        open_index(journal.path())
            .unwrap()
            .query_row("SELECT count(*) FROM edges WHERE path=?", [rel], |row| {
                row.get(0)
            })
            .unwrap()
    };
    assert_eq!(edges(&journal), 0);

    journal.save(
        "entities/alice/entity.json",
        r#"{"name":"Alice Edge","type":"Person","aka":["Ally"]}"#,
    );
    solstone_core_indexer_todo::enqueue_token(
        journal.path(),
        solstone_core_indexer_todo::Token::EntitySearch,
    )
    .unwrap();
    // The entity change marks the day; its refresh waits behind everything
    // else and runs once, last, before the drain ends.
    let report = drain_now(journal.path()).unwrap();
    assert_eq!((report.processed, report.failed), (2, 0));
    assert_eq!(edges(&journal), 1);
    assert!(inspect(journal.path()).unwrap().keys.is_empty());
}

/// Keys in the order this drain attempted them today, with each event.
fn attempts(journal: &Journal) -> Vec<serde_json::Value> {
    let log = journal
        .path()
        .join("chronicle")
        .join(chrono::Utc::now().format("%Y%m%d").to_string())
        .join("health/indexer.jsonl");
    fs::read_to_string(log)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

fn attempts_of(journal: &Journal, key: &str) -> usize {
    attempts(journal)
        .iter()
        .filter(|event| event["path"] == key)
        .count()
}

/// `count` queued removals for days long past: a backlog that is cheap to drain.
fn old_backlog(journal: &Journal, count: usize, tag: &str) {
    for index in 0..count {
        let key = format!(
            "2025{:02}{:02}/desktop/{tag}{index:06}_300/talents/a.md",
            index % 12 + 1,
            index % 28 + 1
        );
        enqueue_path(journal.path(), Path::new(&key)).unwrap();
    }
}

fn forced() -> DrainOptions {
    DrainOptions {
        force: true,
        ..DrainOptions::default()
    }
}

#[test]
fn a_save_during_a_long_backlog_is_served_within_one_snapshot() {
    let journal = Journal::new();
    old_backlog(&journal, 3 * SNAPSHOT_ENTRIES, "1");
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let fresh = format!("{today}/desktop/120000_300/talents/activity.md");
    let saved = std::cell::Cell::new(false);
    let report = drain_with_checkpoint(journal.path(), forced(), &|| {
        // Once the first snapshot is under way.
        if !saved.get() && !attempts(&journal).is_empty() {
            saved.set(true);
            journal.save(&fresh, "a fresh kestrel note");
            enqueue_path(
                journal.path(),
                &journal.path().join("chronicle").join(&fresh),
            )
            .map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(
        (report.processed, report.failed),
        (3 * SNAPSHOT_ENTRIES + 1, 0)
    );
    let order = attempts(&journal);
    let position = order
        .iter()
        .position(|event| event["path"] == fresh.as_str());
    // The first snapshot was already fixed; the save leads the next one.
    assert_eq!(position, Some(SNAPSHOT_ENTRIES));
    // Every search attempt says how long it waited.
    assert!(order.iter().all(|event| event["queued_ms"].is_u64()));
}

#[test]
fn a_failing_token_is_kept_and_served_once_per_drain_under_a_backlog() {
    let journal = Journal::new();
    // Entity search cannot record what it wrote: it fails every time.
    scan_journal(journal.path(), false).unwrap();
    open_index(journal.path())
        .unwrap()
        .execute_batch("CREATE TRIGGER refuse_entity_search BEFORE INSERT ON entity_search_watermark BEGIN SELECT RAISE(ABORT, 'fixture entity search refusal'); END;")
        .unwrap();
    journal.save(
        "entities/alice/entity.json",
        r#"{"name":"Alice Kestrel","type":"Person"}"#,
    );
    enqueue_token(journal.path(), Token::EntitySearch).unwrap();
    for attempt in 1..=3 {
        assert_eq!(drain(journal.path(), forced()).unwrap().failed, 1);
        assert_eq!(listed(journal.path())[0].attempts, attempt);
    }
    // Kept after three failures, waiting an hour, and counted.
    let kept = listed(journal.path()).pop().unwrap();
    assert_eq!(kept.key, "@entity-search");
    assert!(kept.not_before_ms > now_ms() + RETRY_MS);
    assert_eq!(inspect(journal.path()).unwrap().failed_tokens, 1);
    // An ordinary drain leaves it waiting.
    drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(attempts_of(&journal, "@entity-search"), 3);

    // Under a backlog that keeps growing, a forced drain still serves it,
    // once, and returns.
    old_backlog(&journal, 4 * SNAPSHOT_ENTRIES, "1");
    let added = std::cell::Cell::new(0);
    let report = drain_with_checkpoint(journal.path(), forced(), &|| {
        added.set(added.get() + 1);
        if added.get() % 3 == 0 && added.get() < 60 {
            let key = format!("20250101/desktop/9{:06}_300/talents/a.md", added.get());
            enqueue_path(journal.path(), Path::new(&key)).map_err(|error| error.to_string())?;
        }
        Ok(())
    })
    .unwrap();
    assert_eq!(report.failed, 1);
    assert_eq!(attempts_of(&journal, "@entity-search"), 4);
    assert_eq!(listed(journal.path())[0].attempts, 4);
}

#[test]
fn a_kept_token_queued_again_by_its_own_cause_keeps_its_delay() {
    let journal = Journal::new();
    let claims = root(journal.path()).join("claimed");
    fs::create_dir_all(&claims).unwrap();
    // A producer queues the token again while its third failing run holds it.
    enqueue_token(journal.path(), Token::Reclassify).unwrap();
    let mut entry = listed(journal.path()).pop().unwrap();
    entry.attempts = 2;
    let claim = claims.join(entry.filename());
    fs::remove_file(root(journal.path()).join(entry.filename())).unwrap();
    rewrite(&claim, &entry).unwrap();
    enqueue_token(journal.path(), Token::Reclassify).unwrap();
    return_failed(journal.path(), entry.clone(), &claim, "fixture failure").unwrap();
    let kept = listed(journal.path()).pop().unwrap();
    assert_eq!(kept.attempts, 3);
    assert!(kept.not_before_ms > now_ms() + RETRY_MS);

    // A third failure found by claim recovery keeps it too.
    fs::remove_file(root(journal.path()).join(entry.filename())).unwrap();
    rewrite(&claim, &entry).unwrap();
    let report = drain(journal.path(), DrainOptions::default()).unwrap();
    assert_eq!(report.recovered, 1);
    let kept = listed(journal.path()).pop().unwrap();
    assert_eq!((kept.key.as_str(), kept.attempts), ("@reclassify", 3));
}

#[test]
fn connections_run_once_and_last_however_often_they_are_queued() {
    let journal = Journal::new();
    old_backlog(&journal, 40, "1");
    enqueue_token(journal.path(), Token::Edges).unwrap();
    // The refresh re-queues itself; so does this checkpoint, every call.
    let report = drain_with_checkpoint(journal.path(), forced(), &|| {
        enqueue_token(journal.path(), Token::Edges).map_err(|error| error.to_string())
    })
    .unwrap();
    assert_eq!(report.failed, 0);
    let order = attempts(&journal);
    assert_eq!(attempts_of(&journal, "@edges"), 1);
    assert_eq!(order.last().unwrap()["path"], "@edges");
    // A connections refresh waiting is not search lag.
    assert!(order.last().unwrap().get("queued_ms").is_none());
    assert_eq!(inspect(journal.path()).unwrap().keys, ["@edges"]);
}

#[test]
fn a_stop_by_the_drains_own_budget_spends_no_attempt() {
    let journal = Journal::new();
    journal.save(NOTE, "kestrel");
    enqueue_token(journal.path(), Token::ScanFull).unwrap();
    let claims = root(journal.path()).join("claimed");
    // The checkpoint refuses once the token is claimed, as an expiring budget does.
    let report = drain_with_checkpoint(journal.path(), forced(), &|| {
        if fs::read_dir(&claims).is_ok_and(|mut entries| entries.next().is_some()) {
            Err("index work wait expired".into())
        } else {
            Ok(())
        }
    })
    .unwrap();
    assert!(report.timed_out);
    assert_eq!(report.failed, 0);
    let entry = listed(journal.path()).pop().unwrap();
    assert_eq!((entry.key.as_str(), entry.attempts), ("@scan-full", 0));
    assert_eq!(inspect(journal.path()).unwrap().claimed, 0);
}

#[test]
fn a_full_scan_with_one_bad_file_is_done_and_the_file_stays_failed() {
    let journal = Journal::new();
    journal.save("imports/fixture/summary.md", "kestrel");
    fs::write(journal.path().join("imports/fixture/summary.md"), [0xff]).unwrap();
    enqueue_token(journal.path(), Token::ScanFull).unwrap();
    let report = drain(journal.path(), forced()).unwrap();
    assert_eq!((report.processed, report.failed), (1, 0));
    assert!(listed(journal.path()).is_empty());
    let status = crate::status::inspect_index(journal.path()).unwrap();
    assert!(status.failed.contains("imports/fixture/summary.md"));
    // The daily check tries it again.
    assert_eq!(scan_journal(journal.path(), false).unwrap().failed, 1);
}

#[test]
fn a_catch_all_starts_with_the_newest_day() {
    let journal = Journal::new();
    for day in ["20240105", "20261101", "20250610"] {
        journal.save(
            &format!("{day}/desktop/120000_300/talents/activity.md"),
            "kestrel",
        );
    }
    scan_journal(journal.path(), false).unwrap();
    let first: String = open_index(journal.path())
        .unwrap()
        .query_row(
            "SELECT path FROM chunk_sources ORDER BY rowid LIMIT 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert!(first.contains("20261101"), "{first}");
}

/// Run by hand: `cargo test -p solstone-core-indexer-store --features full-tests
/// --lib -- --ignored --nocapture high_water`.
#[test]
#[ignore]
fn high_water_list_cost() {
    let count: usize = std::env::var("HIGH_WATER")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(300_000);
    let journal = Journal::new();
    let started = Instant::now();
    // Written directly: a producer's durable publication flushes each entry,
    // which is the producer's cost, not the list's.
    let directory = root(journal.path());
    fs::create_dir_all(&directory).unwrap();
    for index in 0..count {
        let entry = Entry {
            key: format!(
                "2025{:02}{:02}/desktop/1{index:06}_300/talents/a.md",
                index % 12 + 1,
                index % 28 + 1
            ),
            kind: "path".to_owned(),
            enqueued_ms: now_ms(),
            attempts: 0,
            // Due at any real time, not at time zero: the nothing-due probe
            // below asks at zero, and forced drains ignore the delay.
            not_before_ms: 1,
        };
        fs::write(
            directory.join(entry.filename()),
            serde_json::to_vec(&entry).unwrap(),
        )
        .unwrap();
    }
    eprintln!("write {count} entries: {:?}", started.elapsed());
    let time = |label: &str, run: &dyn Fn()| {
        let started = Instant::now();
        run();
        eprintln!("{label}: {:?}", started.elapsed());
    };
    time("inspect (Health, status)", &|| {
        assert_eq!(inspect(journal.path()).unwrap().pending, count);
    });
    time("has_due, first entry due", &|| {
        assert!(has_due(journal.path(), now_ms()).unwrap());
    });
    time("has_due, nothing due", &|| {
        assert!(!has_due(journal.path(), 0).unwrap());
    });
    // The drain's own list work for one whole pass at the production
    // snapshot size: every listing and selection, no index writes.
    const PRODUCTION_SNAPSHOT: usize = 256;
    let mut list = WorkList::default();
    let started = Instant::now();
    let first = list
        .next_snapshot(journal.path(), true, true, PRODUCTION_SNAPSHOT)
        .unwrap();
    eprintln!(
        "first snapshot (decodes every entry): {:?}",
        started.elapsed()
    );
    for entry in &first {
        list.served(entry);
    }
    let started = Instant::now();
    let mut snapshots = 1;
    loop {
        let next = list
            .next_snapshot(journal.path(), true, true, PRODUCTION_SNAPSHOT)
            .unwrap();
        if next.is_empty() {
            break;
        }
        snapshots += 1;
        for entry in &next {
            list.served(entry);
        }
    }
    eprintln!(
        "remaining {} snapshots of {PRODUCTION_SNAPSHOT}: {:?} (listing and selection only)",
        snapshots - 1,
        started.elapsed()
    );
    let started = Instant::now();
    let report = drain(
        journal.path(),
        DrainOptions {
            force: true,
            budget: Duration::from_secs(60),
        },
    )
    .unwrap();
    eprintln!(
        "60 s forced drain at high water: {} removals applied in {:?}",
        report.processed,
        started.elapsed()
    );
}

#[test]
fn a_file_saved_again_after_its_turn_leads_the_next_drain() {
    let journal = Journal::new();
    old_backlog(&journal, 4 * SNAPSHOT_ENTRIES, "1");
    let today = chrono::Local::now().format("%Y%m%d").to_string();
    let hot = format!("{today}/desktop/120000_300/talents/activity.md");
    let source = journal.save(&hot, "the note the owner is editing");
    enqueue_path(journal.path(), &source).unwrap();
    let saved = std::cell::Cell::new(false);
    let first = drain_with_checkpoint(journal.path(), forced(), &|| {
        if !saved.get() && attempts_of(&journal, &hot) == 1 {
            saved.set(true);
            enqueue_path(journal.path(), &source).map_err(|error| error.to_string())?;
            // Past the drain's slice, so it yields at the next snapshot.
            std::thread::sleep(DRAIN_SLICE + Duration::from_millis(50));
        }
        Ok(())
    })
    .unwrap();
    assert!(first.processed < 4 * SNAPSHOT_ENTRIES + 1, "{first:?}");
    assert!(!first.timed_out);
    drain(journal.path(), forced()).unwrap();
    let order = attempts(&journal);
    assert_eq!(order[0]["path"], hot.as_str());
    assert_eq!(order[first.processed]["path"], hot.as_str());
}
