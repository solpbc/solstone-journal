// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Batch execution, status inspection, and drain loop for chunk classifications.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::json;

use crate::StoreError;
use crate::chunk_sources::{
    CHUNK_SOURCES_LOOKUP_PATHS, CHUNK_SOURCES_LOOKUP_STREAM, chunk_path_lookup_ready,
    require_chunk_path_lookup,
};
use crate::classification::{FacetDeclarationSet, classify_source};
use crate::db::{
    CREATE_CHUNK_CLASSIFICATION, CREATE_CHUNK_CLASSIFICATION_BACKFILL,
    CREATE_CHUNK_CLASSIFICATION_FACETS, CREATE_CHUNK_CLASSIFICATION_FACETS_INDEX,
    ChunkClassificationBackfill, db_path, replace_chunk_classification, sqlite_table_exists,
    write_chunk_classification_backfill,
};

pub const CHUNK_CLASSIFICATION_BACKFILL_STEP: i64 = 32;

static HELD_ONCE: AtomicBool = AtomicBool::new(false);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ClassificationInitialization {
    Absent,
    Unready,
    Incomplete,
    Ready,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ClassificationStatus {
    pub initialization: ClassificationInitialization,
    pub cursor: String,
    pub completed: bool,
    pub stalled: bool,
    pub stalled_path: Option<String>,
    pub processed: usize,
    pub remaining: usize,
    pub missing: usize,
    pub unclassified: usize,
    pub coverage_mismatch: bool,
    pub repair: Option<String>,
    pub(crate) committed: bool,
    pub(crate) stalled_error: Option<String>,
}

impl ClassificationStatus {
    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        let initialization = match self.initialization {
            ClassificationInitialization::Absent => "absent",
            ClassificationInitialization::Unready => "unready",
            ClassificationInitialization::Incomplete => "incomplete",
            ClassificationInitialization::Ready => "ready",
        };
        json!({
            "initialization": initialization,
            "cursor": self.cursor,
            "completed": self.completed,
            "stalled": self.stalled,
            "stalled_path": self.stalled_path,
            "processed": self.processed,
            "remaining": self.remaining,
            "missing": self.missing,
            "unclassified": self.unclassified,
            "coverage_mismatch": self.coverage_mismatch,
            "repair": self.repair,
        })
    }

    #[must_use]
    pub fn format_human(&self) -> String {
        match self.initialization {
            ClassificationInitialization::Absent => {
                "classifications: database absent\n".to_string()
            }
            ClassificationInitialization::Unready => {
                "classifications: unready (run 'solstone-core indexer path-lookup --apply')\n"
                    .to_string()
            }
            ClassificationInitialization::Incomplete => format!(
                "classifications: incomplete sidecar at cursor '{}' (run 'solstone-core indexer classifications --apply')\n",
                self.cursor
            ),
            ClassificationInitialization::Ready => {
                if self.stalled {
                    format!(
                        "classifications: stalled at '{}' (processed {}, remaining {}, missing {}, unclassified {})\n",
                        self.stalled_path.as_deref().unwrap_or(&self.cursor),
                        self.processed,
                        self.remaining,
                        self.missing,
                        self.unclassified
                    )
                } else if self.coverage_mismatch {
                    format!(
                        "classifications: coverage mismatch at cursor '{}' (processed {}, remaining {}, missing {}, unclassified {})\n",
                        self.cursor,
                        self.processed,
                        self.remaining,
                        self.missing,
                        self.unclassified
                    )
                } else if self.completed {
                    format!(
                        "classifications: complete (processed {}, unclassified {})\n",
                        self.processed, self.unclassified
                    )
                } else {
                    format!(
                        "classifications: in progress at cursor '{}' (processed {}, remaining {}, missing {}, unclassified {})\n",
                        self.cursor,
                        self.processed,
                        self.remaining,
                        self.missing,
                        self.unclassified
                    )
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ResumeCount {
    Preserve,
    Set(i64),
}

#[cfg(test)]
#[derive(Default)]
struct TestSeamSlot {
    target_journal: Option<std::path::PathBuf>,
    sql_before_commit: bool,
    hold_before_commit: Option<std::sync::Arc<TestHoldSync>>,
}

#[cfg(test)]
struct TestHoldSync {
    recorded: std::sync::Mutex<Vec<Vec<String>>>,
    cvar: std::sync::Condvar,
    released_batches: std::sync::Mutex<usize>,
}

#[cfg(test)]
static TEST_SEAM: std::sync::Mutex<TestSeamSlot> = std::sync::Mutex::new(TestSeamSlot {
    target_journal: None,
    sql_before_commit: false,
    hold_before_commit: None,
});

#[cfg(test)]
pub(crate) struct TestSeamGuard;

#[cfg(test)]
impl Drop for TestSeamGuard {
    fn drop(&mut self) {
        let mut seam = TEST_SEAM.lock().expect("lock test seam on drop");
        seam.target_journal = None;
        seam.sql_before_commit = false;
        seam.hold_before_commit = None;
    }
}

#[cfg(test)]
pub(crate) fn arm_sql_before_commit(journal: &Path) -> TestSeamGuard {
    let mut seam = TEST_SEAM.lock().expect("lock test seam");
    seam.target_journal = Some(journal.to_path_buf());
    seam.sql_before_commit = true;
    seam.hold_before_commit = None;
    TestSeamGuard
}

#[cfg(test)]
pub(crate) struct HoldController {
    sync: std::sync::Arc<TestHoldSync>,
    _guard: TestSeamGuard,
}

#[cfg(test)]
impl HoldController {
    pub fn wait_for_recorded_window(&self, batch_index: usize) -> Vec<String> {
        let mut rec = self.sync.recorded.lock().expect("lock recorded");
        while rec.len() <= batch_index {
            rec = self.sync.cvar.wait(rec).expect("wait recorded window");
        }
        rec[batch_index].clone()
    }

    pub fn release_next(&self) {
        let mut rel = self
            .sync
            .released_batches
            .lock()
            .expect("lock released_batches");
        *rel += 1;
        self.sync.cvar.notify_all();
    }
}

#[cfg(test)]
pub(crate) fn arm_hold_before_commit(journal: &Path) -> HoldController {
    let sync = std::sync::Arc::new(TestHoldSync {
        recorded: std::sync::Mutex::new(Vec::new()),
        cvar: std::sync::Condvar::new(),
        released_batches: std::sync::Mutex::new(0),
    });
    let mut seam = TEST_SEAM.lock().expect("lock test seam");
    seam.target_journal = Some(journal.to_path_buf());
    seam.sql_before_commit = false;
    seam.hold_before_commit = Some(sync.clone());
    HoldController {
        sync,
        _guard: TestSeamGuard,
    }
}

fn count_remaining(conn: &Connection, cursor: &str) -> Result<usize, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(DISTINCT path) FROM chunk_sources WHERE path IS NOT NULL AND path != '' AND path > ?",
        [cursor],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

fn count_missing_internal(
    conn: &Connection,
    has_classification_table: bool,
) -> Result<usize, StoreError> {
    if has_classification_table {
        let count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT s.path) FROM chunk_sources s LEFT JOIN chunk_classification cc ON cc.path = s.path WHERE s.path IS NOT NULL AND s.path != '' AND cc.path IS NULL",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    } else {
        let count: i64 = conn.query_row(
            "SELECT COUNT(DISTINCT path) FROM chunk_sources WHERE path IS NOT NULL AND path != ''",
            [],
            |row| row.get(0),
        )?;
        Ok(count as usize)
    }
}

fn count_unclassified(conn: &Connection) -> Result<usize, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM chunk_classification WHERE unclassified = 1",
        [],
        |row| row.get(0),
    )?;
    Ok(count as usize)
}

pub fn inspect_classifications(journal: &Path) -> Result<ClassificationStatus, StoreError> {
    let path = db_path(journal);
    if !path.is_file() {
        return Ok(ClassificationStatus {
            initialization: ClassificationInitialization::Absent,
            cursor: String::new(),
            completed: false,
            stalled: false,
            stalled_path: None,
            processed: 0,
            remaining: 0,
            missing: 0,
            unclassified: 0,
            coverage_mismatch: false,
            repair: None,
            committed: false,
            stalled_error: None,
        });
    }

    let conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !chunk_path_lookup_ready(&conn)? {
        return Ok(ClassificationStatus {
            initialization: ClassificationInitialization::Unready,
            cursor: String::new(),
            completed: false,
            stalled: false,
            stalled_path: None,
            processed: 0,
            remaining: 0,
            missing: 0,
            unclassified: 0,
            coverage_mismatch: false,
            repair: Some("indexer path-lookup --apply".to_string()),
            committed: false,
            stalled_error: None,
        });
    }

    let has_classification_table = sqlite_table_exists(&conn, "chunk_classification")?;
    let has_backfill_table = sqlite_table_exists(&conn, "chunk_classification_backfill")?;
    let backfill = if has_backfill_table {
        conn.query_row(
            "SELECT cursor, completed, stalled, stalled_path, resume_count FROM chunk_classification_backfill WHERE id=1",
            [],
            |row| {
                Ok(ChunkClassificationBackfill {
                    cursor: row.get(0)?,
                    completed: row.get::<_, i64>(1)? != 0,
                    stalled: row.get::<_, i64>(2)? != 0,
                    stalled_path: row.get(3)?,
                    resume_count: row.get(4)?,
                })
            },
        ).optional()?
    } else {
        None
    };

    if backfill.is_some() && !has_classification_table {
        let cursor = backfill
            .as_ref()
            .map(|b| b.cursor.clone())
            .unwrap_or_default();
        let stalled = backfill.as_ref().is_some_and(|b| b.stalled);
        let stalled_path = backfill.as_ref().and_then(|b| b.stalled_path.clone());
        let completed = backfill.as_ref().is_some_and(|b| b.completed);
        let remaining = count_remaining(&conn, &cursor)?;
        let missing = count_missing_internal(&conn, false)?;
        let unclassified = 0;
        let coverage_mismatch = (completed || remaining == 0) && missing > 0;
        return Ok(ClassificationStatus {
            initialization: ClassificationInitialization::Incomplete,
            cursor,
            completed,
            stalled,
            stalled_path,
            processed: 0,
            remaining,
            missing,
            unclassified,
            coverage_mismatch,
            repair: Some("classifications --apply".to_string()),
            committed: false,
            stalled_error: None,
        });
    }

    let cursor = backfill
        .as_ref()
        .map(|b| b.cursor.clone())
        .unwrap_or_default();
    let completed = backfill.as_ref().is_some_and(|b| b.completed);
    let stalled = backfill.as_ref().is_some_and(|b| b.stalled);
    let stalled_path = backfill.as_ref().and_then(|b| b.stalled_path.clone());
    let remaining = count_remaining(&conn, &cursor)?;
    let missing = count_missing_internal(&conn, has_classification_table)?;
    let unclassified = if has_classification_table {
        count_unclassified(&conn)?
    } else {
        0
    };
    let coverage_mismatch = (completed || remaining == 0) && missing > 0;

    Ok(ClassificationStatus {
        initialization: ClassificationInitialization::Ready,
        cursor,
        completed,
        stalled,
        stalled_path,
        processed: 0,
        remaining,
        missing,
        unclassified,
        coverage_mismatch,
        repair: None,
        committed: false,
        stalled_error: None,
    })
}

pub(crate) fn classify_one_batch(
    conn: &mut Connection,
    journal: &Path,
    resume: ResumeCount,
) -> Result<ClassificationStatus, StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    require_chunk_path_lookup(&tx)?;

    let has_classification = sqlite_table_exists(&tx, "chunk_classification")?;
    let has_facets = sqlite_table_exists(&tx, "chunk_classification_facets")?;
    let has_backfill = sqlite_table_exists(&tx, "chunk_classification_backfill")?;
    let schema_needed = !has_classification || !has_facets || !has_backfill;

    if schema_needed {
        tx.execute(CREATE_CHUNK_CLASSIFICATION, [])?;
        tx.execute(CREATE_CHUNK_CLASSIFICATION_FACETS, [])?;
        tx.execute(CREATE_CHUNK_CLASSIFICATION_FACETS_INDEX, [])?;
        tx.execute(CREATE_CHUNK_CLASSIFICATION_BACKFILL, [])?;
    }

    let existing: Option<ChunkClassificationBackfill> = tx
        .query_row(
            "SELECT cursor, completed, stalled, stalled_path, resume_count FROM chunk_classification_backfill WHERE id=1",
            [],
            |row| {
                Ok(ChunkClassificationBackfill {
                    cursor: row.get(0)?,
                    completed: row.get::<_, i64>(1)? != 0,
                    stalled: row.get::<_, i64>(2)? != 0,
                    stalled_path: row.get(3)?,
                    resume_count: row.get(4)?,
                })
            },
        )
        .optional()?;

    let mut state = existing.clone().unwrap_or(ChunkClassificationBackfill {
        cursor: String::new(),
        completed: false,
        stalled: false,
        stalled_path: None,
        resume_count: 0,
    });

    match resume {
        ResumeCount::Preserve => {}
        ResumeCount::Set(n) => {
            state.resume_count = n;
        }
    }

    if state.completed {
        let remaining = count_remaining(&tx, &state.cursor)?;
        let missing = count_missing_internal(&tx, true)?;
        let unclassified = count_unclassified(&tx)?;
        let coverage_mismatch = missing > 0;
        let committed = if schema_needed {
            tx.commit()?;
            true
        } else {
            let _ = tx.rollback();
            false
        };
        return Ok(ClassificationStatus {
            initialization: ClassificationInitialization::Ready,
            cursor: state.cursor,
            completed: true,
            stalled: state.stalled,
            stalled_path: state.stalled_path,
            processed: 0,
            remaining,
            missing,
            unclassified,
            coverage_mismatch,
            repair: None,
            committed,
            stalled_error: None,
        });
    }

    let declarations = match FacetDeclarationSet::from_journal(journal) {
        Ok(declarations) => declarations,
        Err(error) => {
            let error_str = error.to_string();
            state.stalled = true;
            state.stalled_path = Some(state.cursor.clone());
            write_chunk_classification_backfill(&tx, &state)?;
            tx.commit()?;
            let remaining = count_remaining(conn, &state.cursor)?;
            let missing = count_missing_internal(conn, true)?;
            let unclassified = count_unclassified(conn)?;
            return Ok(ClassificationStatus {
                initialization: ClassificationInitialization::Ready,
                cursor: state.cursor,
                completed: false,
                stalled: true,
                stalled_path: state.stalled_path,
                processed: 0,
                remaining,
                missing,
                unclassified,
                coverage_mismatch: false,
                repair: None,
                committed: true,
                stalled_error: Some(error_str),
            });
        }
    };

    let paths = {
        let mut statement = tx.prepare(CHUNK_SOURCES_LOOKUP_PATHS)?;
        statement
            .query_map(
                [
                    &state.cursor,
                    &CHUNK_CLASSIFICATION_BACKFILL_STEP.to_string(),
                ],
                |row| row.get::<_, String>(0),
            )?
            .collect::<Result<Vec<_>, _>>()?
    };

    #[cfg(test)]
    {
        let hold_opt = {
            let seam = TEST_SEAM.lock().expect("lock test seam");
            if seam.target_journal.as_deref() == Some(journal) {
                seam.hold_before_commit.clone()
            } else {
                None
            }
        };
        if let Some(hold) = hold_opt {
            let batch_index = {
                let mut rec = hold.recorded.lock().expect("lock recorded");
                let idx = rec.len();
                rec.push(paths.clone());
                hold.cvar.notify_all();
                idx
            };
            let mut rel = hold.released_batches.lock().expect("lock released_batches");
            while *rel <= batch_index {
                rel = hold.cvar.wait(rel).expect("wait release");
            }
        }
    }

    let mut processed = 0;
    for path in &paths {
        let stream: Option<String> =
            tx.query_row(CHUNK_SOURCES_LOOKUP_STREAM, [path], |row| row.get(0))?;
        let classification = classify_source(journal, path, stream.as_deref(), &declarations);
        replace_chunk_classification(&tx, &classification)?;
        state.cursor = path.clone();
        state.stalled = false;
        state.stalled_path = None;
        processed += 1;
    }

    let remaining = count_remaining(&tx, &state.cursor)?;
    if remaining == 0 {
        state.completed = true;
        state.stalled = false;
        state.stalled_path = None;
    }

    write_chunk_classification_backfill(&tx, &state)?;
    let missing = count_missing_internal(&tx, true)?;
    let unclassified = count_unclassified(&tx)?;
    let coverage_mismatch = state.completed && missing > 0;

    #[cfg(test)]
    {
        let seam = TEST_SEAM.lock().expect("lock test seam");
        if seam.sql_before_commit && seam.target_journal.as_deref() == Some(journal) {
            tx.execute(
                "INSERT INTO non_existent_table_for_test_fault VALUES(1)",
                [],
            )?;
        }
    }

    tx.commit()?;

    Ok(ClassificationStatus {
        initialization: ClassificationInitialization::Ready,
        cursor: state.cursor,
        completed: state.completed,
        stalled: state.stalled,
        stalled_path: state.stalled_path,
        processed,
        remaining,
        missing,
        unclassified,
        coverage_mismatch,
        repair: None,
        committed: true,
        stalled_error: None,
    })
}

pub fn apply_classification_batch(journal: &Path) -> Result<ClassificationStatus, StoreError> {
    let path = db_path(journal);
    if !path.is_file() {
        return Err(StoreError::MissingFile(path));
    }

    let ro_conn = Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    if !chunk_path_lookup_ready(&ro_conn)? {
        return Err(StoreError::PathLookupRequired { cause: None });
    }
    drop(ro_conn);

    let mut conn = Connection::open(&path)?;
    conn.execute_batch(
        "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    )?;

    let status = classify_one_batch(&mut conn, journal, ResumeCount::Preserve)?;
    drop(conn);

    if status.committed {
        log::info!(
            "classification batch processed={} remaining={} completed={} stalled={} coverage_mismatch={}",
            status.processed,
            status.remaining,
            status.completed,
            status.stalled,
            status.coverage_mismatch
        );

        if !HELD_ONCE.swap(true, Ordering::SeqCst)
            && let Ok(hold_path) = std::env::var("SOLSTONE_INDEXER_CLASSIFICATION_BATCH_HOLD")
            && let Ok(mut file) = std::fs::File::open(&hold_path)
        {
            use std::io::Read;
            let mut buf = [0u8; 1];
            let _ = file.read_exact(&mut buf);
        }
    }

    Ok(status)
}

pub fn drain_classifications(journal: &Path) -> Result<ClassificationStatus, StoreError> {
    let mut total_processed = 0;
    loop {
        let status = apply_classification_batch(journal)?;
        total_processed += status.processed;
        if status.stalled || status.coverage_mismatch || status.completed {
            let mut final_status = status;
            final_status.processed = total_processed;
            return Ok(final_status);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-store-{name}"))
    }

    #[test]
    fn classification_batch_inspect_missing_database_creates_no_files() {
        let root = temp_root("inspect-missing");
        let status = inspect_classifications(&root).expect("inspect missing");
        assert_eq!(status.initialization, ClassificationInitialization::Absent);
        assert!(!root.join("indexer").exists());
        assert!(!db_path(&root).exists());
    }

    #[test]
    fn classification_batch_inspect_ready_database_is_immutable() {
        let root = temp_root("inspect-ready");
        let conn = crate::db::open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(content, path) VALUES ('sample', 'sample.md')",
            [],
        )
        .expect("seed");
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        let status = inspect_classifications(&root).expect("inspect ready");
        assert_eq!(status.initialization, ClassificationInitialization::Ready);
        assert_eq!(status.remaining, 1);
        assert_eq!(status.missing, 1);
        assert!(!status.completed);
        assert_eq!(status.repair, None);

        let inspect_conn =
            Connection::open_with_flags(db_path(&root), OpenFlags::SQLITE_OPEN_READ_ONLY)
                .expect("open read only");
        let backfill =
            crate::db::read_chunk_classification_backfill(&inspect_conn).expect("read backfill");
        assert!(backfill.is_none());
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_saved_cursor_with_dropped_classification_table() {
        let root = temp_root("incomplete-sidecar");
        let conn = crate::db::open_index(&root).expect("open");
        for i in 0..50 {
            conn.execute(
                "INSERT INTO chunks(content, path) VALUES ('text', ?1)",
                [format!("doc-{i:02}.md")],
            )
            .expect("seed");
        }
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        // Run one batch (32 items) to advance cursor to doc-31.md.
        let status1 = apply_classification_batch(&root).expect("apply batch 1");
        assert_eq!(status1.processed, 32);
        assert_eq!(status1.cursor, "doc-31.md");
        let saved_cursor = status1.cursor.clone();

        // Drop chunk_classification table directly to create incomplete sidecar.
        let raw_conn = Connection::open(db_path(&root)).expect("open raw");
        raw_conn
            .execute("DROP TABLE chunk_classification", [])
            .expect("drop table");
        drop(raw_conn);

        // Inspect reports Incomplete.
        let inspect_status = inspect_classifications(&root).expect("inspect incomplete");
        assert_eq!(
            inspect_status.initialization,
            ClassificationInitialization::Incomplete
        );
        assert_eq!(inspect_status.cursor, saved_cursor);
        assert_eq!(
            inspect_status.repair,
            Some("classifications --apply".to_string())
        );

        // Apply recreates table and resumes from saved_cursor without resetting to ''.
        let apply_status = apply_classification_batch(&root).expect("apply resume");
        assert_eq!(apply_status.processed, 18);
        assert_eq!(apply_status.cursor, "doc-49.md");
        assert!(apply_status.cursor > saved_cursor);
        assert!(apply_status.completed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_writes_at_most_32_paths_and_resolves_lowest_rowid_stream() {
        let root = temp_root("batch-limit-and-lowest-stream");
        let conn = crate::db::open_index(&root).expect("open");
        // Insert 40 chunks
        for i in 0..40 {
            conn.execute(
                "INSERT INTO chunks(content, path) VALUES ('text', ?1)",
                [format!("path-{i:02}.md")],
            )
            .expect("seed");
        }
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        // For path-00.md, insert two chunk_sources rows: lowest rowid with stream 'mcp.agent', higher with 'transcripts'
        let raw_conn = Connection::open(db_path(&root)).expect("open raw");
        let max_rowid: i64 = raw_conn
            .query_row("SELECT MAX(rowid) FROM chunk_sources", [], |row| row.get(0))
            .unwrap_or(0);
        raw_conn
            .execute("DELETE FROM chunk_sources WHERE path = 'path-00.md'", [])
            .expect("delete");
        raw_conn
            .execute(
                "INSERT INTO chunk_sources(rowid, path, stream) VALUES (?1, 'path-00.md', 'mcp.agent')",
                [max_rowid + 1],
            )
            .expect("insert mcp");
        raw_conn
            .execute(
                "INSERT INTO chunk_sources(rowid, path, stream) VALUES (?1, 'path-00.md', 'transcripts')",
                [max_rowid + 2],
            )
            .expect("insert transcripts");
        drop(raw_conn);

        let status = apply_classification_batch(&root).expect("apply batch");
        assert_eq!(status.processed, 32);
        assert!(!status.completed);
        assert_eq!(status.remaining, 8);

        // Check path-00.md classification row: mcp.agent stream leads to excluded (eligible=0, unclassified=0)
        let verify_conn = Connection::open(db_path(&root)).expect("open verify");
        let (eligible, unclassified): (i64, i64) = verify_conn
            .query_row(
                "SELECT eligible, unclassified FROM chunk_classification WHERE path = 'path-00.md'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("query path-00");
        assert_eq!(eligible, 0);
        assert_eq!(unclassified, 0);

        // Second batch processes the remaining 8 paths and sets completed.
        let status2 = apply_classification_batch(&root).expect("apply batch 2");
        assert_eq!(status2.processed, 8);
        assert!(status2.completed);
        assert_eq!(status2.remaining, 0);
        assert_eq!(status2.missing, 0);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_concurrent_hold_seam_preserves_cursor_ordering() {
        let root = temp_root("concurrent-hold");
        let conn = crate::db::open_index(&root).expect("open");
        for i in 0..50 {
            conn.execute(
                "INSERT INTO chunks(content, path) VALUES ('text', ?1)",
                [format!("item-{i:02}.md")],
            )
            .expect("seed");
        }
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        let controller = arm_hold_before_commit(&root);
        let root_a = root.clone();
        let thread_a = std::thread::spawn(move || {
            apply_classification_batch(&root_a).expect("thread a batch")
        });

        // Wait for thread A's recorded window (32 items)
        let window_a = controller.wait_for_recorded_window(0);
        assert_eq!(window_a.len(), 32);
        let last_a = window_a.last().unwrap().clone();

        // Start thread B while thread A is held inside its transaction
        let root_b = root.clone();
        let thread_b = std::thread::spawn(move || {
            apply_classification_batch(&root_b).expect("thread b batch")
        });

        // Release thread A so it can commit and finish
        controller.release_next();
        let status_a = thread_a.join().expect("join thread a");
        assert_eq!(status_a.cursor, last_a);
        assert_eq!(status_a.processed, 32);

        // Thread B should unblock and record its window (18 items)
        let window_b = controller.wait_for_recorded_window(1);
        assert_eq!(window_b.len(), 18);
        assert!(window_b.iter().all(|p| p > &last_a));

        // Release thread B so it can commit and finish
        controller.release_next();
        let status_b = thread_b.join().expect("join thread b");
        assert_eq!(status_b.processed, 18);
        assert!(status_b.cursor > last_a);
        assert!(status_b.completed);

        // Verify database state: cursor matches B's last path, 50 rows total
        let verify_conn = Connection::open(db_path(&root)).expect("open verify");
        let backfill = crate::db::read_chunk_classification_backfill(&verify_conn)
            .expect("read backfill")
            .expect("backfill exists");
        assert!(backfill.completed);
        assert_eq!(backfill.cursor, "item-49.md");
        let count: i64 = verify_conn
            .query_row("SELECT count(*) FROM chunk_classification", [], |row| {
                row.get(0)
            })
            .expect("count rows");
        assert_eq!(count, 50);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_sql_fault_rolls_back_cursor_and_classifications() {
        let root = temp_root("sql-fault");
        let conn = crate::db::open_index(&root).expect("open");
        for i in 0..50 {
            conn.execute(
                "INSERT INTO chunks(content, path) VALUES ('text', ?1)",
                [format!("doc-{i:02}.md")],
            )
            .expect("seed");
        }
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        // First batch succeeds (32 items)
        let status1 = apply_classification_batch(&root).expect("apply batch 1");
        assert_eq!(status1.processed, 32);
        assert_eq!(status1.cursor, "doc-31.md");

        // Arm SQL fault for second batch
        {
            let _guard = arm_sql_before_commit(&root);
            let result = apply_classification_batch(&root);
            assert!(result.is_err());
        }

        // Verify DB cursor is still doc-31.md and row count is still 32
        let verify_conn = Connection::open(db_path(&root)).expect("open verify");
        let backfill = crate::db::read_chunk_classification_backfill(&verify_conn)
            .expect("read backfill")
            .expect("backfill exists");
        assert_eq!(backfill.cursor, "doc-31.md");
        let count: i64 = verify_conn
            .query_row("SELECT count(*) FROM chunk_classification", [], |row| {
                row.get(0)
            })
            .expect("count rows");
        assert_eq!(count, 32);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_facet_directory_stall_and_resume() {
        let root = temp_root("facet-stall");
        let conn = crate::db::open_index(&root).expect("open");
        conn.execute(
            "INSERT INTO chunks(content, path) VALUES ('text', 'sample.md')",
            [],
        )
        .expect("seed");
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        // Write a file at facets path to cause directory error
        fs::write(root.join("facets"), "not a directory").expect("write facets file");

        let status = apply_classification_batch(&root).expect("apply stalled");
        assert!(status.stalled);
        assert_eq!(status.cursor, "");
        assert_eq!(status.stalled_path, Some(String::new()));

        // Remove file and make it a valid directory
        fs::remove_file(root.join("facets")).expect("remove facets file");
        fs::create_dir_all(root.join("facets")).expect("create facets dir");

        let status2 = apply_classification_batch(&root).expect("apply resumed");
        assert!(!status2.stalled);
        assert!(status2.completed);
        assert_eq!(status2.cursor, "sample.md");
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    #[cfg(unix)]
    fn classification_batch_unreadable_facet_json_stores_unclassified() {
        use std::os::unix::fs::PermissionsExt;

        let root = temp_root("unreadable-facet-json");
        let conn = crate::db::open_index(&root).expect("open");
        conn.execute(
            "INSERT INTO chunks(content, path) VALUES ('text', 'facets/Secret/news/20260101.md')",
            [],
        )
        .expect("seed secret");
        conn.execute(
            "INSERT INTO chunks(content, path) VALUES ('text', 'facets/Public/news/20260101.md')",
            [],
        )
        .expect("seed public");
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        // Create Public facet with valid JSON
        fs::create_dir_all(root.join("facets/Public")).expect("create Public dir");
        fs::write(
            root.join("facets/Public/facet.json"),
            r#"{"id":"0123456789abcdef01234567"}"#,
        )
        .expect("write Public json");

        // Create Secret facet with unreadable JSON
        fs::create_dir_all(root.join("facets/Secret")).expect("create Secret dir");
        let secret_json = root.join("facets/Secret/facet.json");
        fs::write(&secret_json, r#"{"id":"0123456789abcdef01234568"}"#).expect("write Secret json");
        fs::set_permissions(&secret_json, fs::Permissions::from_mode(0o000))
            .expect("make unreadable");

        let status = apply_classification_batch(&root).expect("apply batch");
        assert!(status.completed);
        assert_eq!(status.processed, 2);
        assert_eq!(status.unclassified, 1);
        assert_eq!(status.missing, 0);
        assert!(!status.stalled);

        // Restore permissions for cleanup
        let _ = fs::set_permissions(&secret_json, fs::Permissions::from_mode(0o644));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_coverage_mismatch_on_completed_with_missing_gap() {
        let root = temp_root("coverage-mismatch");
        let conn = crate::db::open_index(&root).expect("open");
        conn.execute(
            "INSERT INTO chunks(content, path) VALUES ('text', 'a.md'), ('text', 'b.md')",
            [],
        )
        .expect("seed");
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        // Run drain to complete
        let status = drain_classifications(&root).expect("drain");
        assert!(status.completed);
        assert_eq!(status.missing, 0);
        assert!(!status.coverage_mismatch);

        // Delete classification for a.md while keeping backfill completed
        let verify_conn = Connection::open(db_path(&root)).expect("open verify");
        verify_conn
            .execute("DELETE FROM chunk_classification WHERE path = 'a.md'", [])
            .expect("delete row");
        drop(verify_conn);

        let inspect_status = inspect_classifications(&root).expect("inspect");
        assert!(inspect_status.completed);
        assert_eq!(inspect_status.missing, 1);
        assert!(inspect_status.coverage_mismatch);

        let apply_status = apply_classification_batch(&root).expect("apply");
        assert!(apply_status.completed);
        assert_eq!(apply_status.missing, 1);
        assert!(apply_status.coverage_mismatch);
        assert_eq!(apply_status.cursor, "b.md");
        assert_eq!(apply_status.cursor, inspect_status.cursor);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_empty_and_already_complete_truthful_zeros() {
        let root = temp_root("empty-population");
        let conn = crate::db::open_index(&root).expect("open");
        drop(conn);
        crate::chunk_sources::apply_path_lookup(&root).expect("apply path lookup");

        let status = apply_classification_batch(&root).expect("apply empty");
        assert_eq!(status.processed, 0);
        assert!(status.completed);
        assert_eq!(status.remaining, 0);
        assert_eq!(status.missing, 0);
        assert_eq!(status.unclassified, 0);

        let status2 = apply_classification_batch(&root).expect("apply again");
        assert_eq!(status2.processed, 0);
        assert!(status2.completed);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn classification_batch_json_formatter_fields() {
        let status = ClassificationStatus {
            initialization: ClassificationInitialization::Ready,
            cursor: "test/path.md".to_string(),
            completed: true,
            stalled: false,
            stalled_path: None,
            processed: 15,
            remaining: 0,
            missing: 0,
            unclassified: 2,
            coverage_mismatch: false,
            repair: None,
            committed: false,
            stalled_error: None,
        };
        let value = status.to_json_value();
        assert_eq!(value["initialization"], "ready");
        assert_eq!(value["cursor"], "test/path.md");
        assert_eq!(value["completed"], true);
        assert_eq!(value["stalled"], false);
        assert_eq!(value["stalled_path"], serde_json::Value::Null);
        assert_eq!(value["processed"], 15);
        assert_eq!(value["remaining"], 0);
        assert_eq!(value["missing"], 0);
        assert_eq!(value["unclassified"], 2);
        assert_eq!(value["coverage_mismatch"], false);
        assert_eq!(value["repair"], serde_json::Value::Null);
    }
}
