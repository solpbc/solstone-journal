// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Mapping table and admission marker for index chunk path lookups.

use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use crate::StoreError;
use crate::db::{db_path, migrate_legacy_chunks, sqlite_table_exists};
use crate::writer_admission::IndexAdmission;

pub const CREATE_CHUNK_SOURCES: &str = "\
CREATE TABLE IF NOT EXISTS chunk_sources(
rowid INTEGER PRIMARY KEY,
path TEXT,
stream TEXT
)";

pub const CREATE_CHUNK_SOURCES_PATH_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS chunk_sources_path ON chunk_sources(path)";

pub const CREATE_CHUNK_SOURCE_READINESS: &str = "\
CREATE TABLE IF NOT EXISTS chunk_source_readiness(
id INTEGER PRIMARY KEY CHECK (id = 1),
ready INTEGER NOT NULL CHECK (ready IN (0, 1))
)";

pub const CHUNK_SOURCES_LOOKUP_ROWIDS: &str = "SELECT rowid FROM chunk_sources WHERE path = ?";

pub const CHUNK_SOURCES_LOOKUP_STREAM: &str =
    "SELECT stream FROM chunk_sources WHERE path = ? ORDER BY rowid ASC LIMIT 1";

pub const CHUNK_SOURCES_LOOKUP_PATHS: &str =
    "SELECT DISTINCT path FROM chunk_sources WHERE path > ? ORDER BY path ASC LIMIT ?";

const VALIDATE_CHUNK_SOURCES_CORRESPONDENCE: &str = "\
SELECT 1 WHERE EXISTS (
  SELECT 1 FROM chunks AS c
  LEFT JOIN chunk_sources AS s ON s.rowid = c.rowid
  WHERE s.rowid IS NULL OR s.path IS NOT c.path OR s.stream IS NOT c.stream
) OR EXISTS (
  SELECT 1 FROM chunk_sources AS s
  LEFT JOIN chunks AS c ON c.rowid = s.rowid
  WHERE c.rowid IS NULL OR c.path IS NOT s.path OR c.stream IS NOT s.stream
)";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PathLookupStatus {
    pub ready: bool,
}

pub(crate) fn record_chunk_source(
    conn: &Connection,
    rowid: Option<i64>,
    path: Option<&str>,
    stream: Option<&str>,
) -> Result<(), StoreError> {
    let rowid = rowid.unwrap_or_else(|| conn.last_insert_rowid());
    conn.execute(
        "INSERT OR REPLACE INTO chunk_sources(rowid, path, stream) VALUES (?, ?, ?)",
        params![rowid, path, stream],
    )?;
    Ok(())
}

pub(crate) fn delete_chunk_source_rowids(
    conn: &Connection,
    rowids: &[i64],
) -> Result<(), StoreError> {
    if rowids.is_empty() || !sqlite_table_exists(conn, "chunk_sources")? {
        return Ok(());
    }
    let mut stmt = conn.prepare_cached("DELETE FROM chunk_sources WHERE rowid = ?")?;
    for rowid in rowids {
        stmt.execute([rowid])?;
    }
    Ok(())
}

pub(crate) fn chunk_path_lookup_ready(conn: &Connection) -> Result<bool, StoreError> {
    if !sqlite_table_exists(conn, "chunk_source_readiness")? {
        return Ok(false);
    }
    let columns = {
        let mut statement = conn.prepare("PRAGMA table_info(chunk_source_readiness)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
    };
    if !columns.iter().any(|c| c == "ready") {
        return Ok(false);
    }
    let ready = conn
        .query_row(
            "SELECT ready FROM chunk_source_readiness WHERE id = 1",
            [],
            |row| row.get::<_, i64>(0),
        )
        .optional()?;
    Ok(ready == Some(1))
}

pub(crate) fn require_chunk_path_lookup(conn: &Connection) -> Result<(), StoreError> {
    if chunk_path_lookup_ready(conn)? {
        Ok(())
    } else {
        Err(StoreError::PathLookupRequired { cause: None })
    }
}

pub fn inspect_path_lookup(journal: &Path) -> Result<PathLookupStatus, StoreError> {
    let path = db_path(journal);
    if !path.is_file() {
        return Ok(PathLookupStatus { ready: false });
    }
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let ready = chunk_path_lookup_ready(&conn)?;
    Ok(PathLookupStatus { ready })
}

pub fn apply_path_lookup(journal: &Path) -> Result<PathLookupStatus, StoreError> {
    let _admission = IndexAdmission::acquire(journal, "path-lookup")?;
    let path = db_path(journal);
    if !path.is_file() {
        return Err(StoreError::PathLookupRequired {
            cause: Some("index database is absent".to_owned()),
        });
    }
    let mut conn = Connection::open(&path)?;
    conn.execute_batch(
        "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    )?;
    seed_path_lookup(&mut conn)?;
    Ok(PathLookupStatus { ready: true })
}

/// Rebuild the path lookup from `chunks` and mark it ready. The caller must
/// hold the index writer admission.
pub(crate) fn seed_path_lookup(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;

    if !sqlite_table_exists(&tx, "chunks")? {
        return Err(StoreError::PathLookupRequired {
            cause: Some("chunks table is absent".to_owned()),
        });
    }

    migrate_legacy_chunks(&tx)?;

    tx.execute(CREATE_CHUNK_SOURCES, [])?;
    tx.execute(CREATE_CHUNK_SOURCES_PATH_INDEX, [])?;
    tx.execute(CREATE_CHUNK_SOURCE_READINESS, [])?;

    tx.execute("DELETE FROM chunk_sources", [])?;
    tx.execute(
        "INSERT INTO chunk_sources(rowid, path, stream) SELECT rowid, path, stream FROM chunks",
        [],
    )?;

    let has_difference: Option<i64> = tx
        .query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |row| row.get(0))
        .optional()?;

    if has_difference.is_some() {
        return Err(StoreError::PathLookupRequired {
            cause: Some(
                "chunk_sources does not correspond to chunks by rowid, path, and stream".to_owned(),
            ),
        });
    }

    tx.execute(
        "REPLACE INTO chunk_source_readiness(id, ready) VALUES (1, 1)",
        [],
    )?;
    tx.commit()?;
    Ok(())
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use super::*;
    use crate::classification::{FacetDeclarationSet, classify_source};
    use crate::db::{
        next_unclassified_chunk_paths, open_index, prune_authored_chat_paths, prune_by_paths,
        prune_chunks_by_stream, reset_index,
    };
    use crate::reconcile::reconcile_stale_classifications;
    use crate::scan::{rebuild_edges, rescan_file, scan_journal};
    use crate::test_support::reserve_temp_path;
    use rusqlite::params;
    use std::fs;
    use std::path::PathBuf;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-store-chunk-sources-{name}"))
    }

    #[test]
    fn fresh_index_from_open_index_is_ready() {
        let root = temp_root("fresh-index");
        let conn = open_index(&root).expect("open fresh index");
        assert!(chunk_path_lookup_ready(&conn).expect("check ready"));
        let ready_val: i64 = conn
            .query_row(
                "SELECT ready FROM chunk_source_readiness WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("read readiness");
        assert_eq!(ready_val, 1);
        drop(conn);

        let status = inspect_path_lookup(&root).expect("inspect status");
        assert!(status.ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn fts_schema_sql_in_sqlite_master_is_unchanged() {
        let root = temp_root("fts-schema-check");
        let index_dir = root.join("indexer");
        fs::create_dir_all(&index_dir).expect("create dir");
        let conn = Connection::open(db_path(&root)).expect("open");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE chunks USING fts5(
                content,
                path UNINDEXED,
                day UNINDEXED,
                facet UNINDEXED,
                agent UNINDEXED,
                stream UNINDEXED,
                idx UNINDEXED,
                time_bucket UNINDEXED
            );
            INSERT INTO chunks(rowid, content, path, day, facet, agent, stream, idx, time_bucket)
            VALUES (42, 'this contains target text', 'path/to/doc.md', '20260101', 'work', 'agent', 'stream-a', 0, 100);",
        )
        .expect("seed");
        let sql_before: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
                [],
                |row| row.get(0),
            )
            .expect("get sql before");
        drop(conn);

        let opened_conn = open_index(&root).expect("open index");
        let sql_after: String = opened_conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='chunks'",
                [],
                |row| row.get(0),
            )
            .expect("get sql after");
        assert_eq!(sql_before, sql_after);

        let (rowid, content): (i64, String) = opened_conn
            .query_row(
                "SELECT rowid, content FROM chunks WHERE chunks MATCH 'target'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("match target");
        assert_eq!(rowid, 42);
        assert_eq!(content, "this contains target text");

        let ready = chunk_path_lookup_ready(&opened_conn).expect("read readiness");
        assert!(!ready);
        drop(opened_conn);

        let status = inspect_path_lookup(&root).expect("inspect");
        assert!(!status.ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn sparse_rowid_legacy_migration_populates_mapping_and_readiness() {
        let root = temp_root("sparse-rowid");
        let index_dir = root.join("indexer");
        fs::create_dir_all(&index_dir).expect("create dir");
        let conn = Connection::open(db_path(&root)).expect("open");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE chunks USING fts5(
                content,
                path UNINDEXED,
                day UNINDEXED,
                facet UNINDEXED,
                agent UNINDEXED,
                stream UNINDEXED,
                idx UNINDEXED
            );
            INSERT INTO chunks(rowid, content, path, stream) VALUES (10, 'c1', 'p1', 's1');
            INSERT INTO chunks(rowid, content, path, stream) VALUES (500, 'c2', NULL, NULL);
            INSERT INTO chunks(rowid, content, path, stream) VALUES (1000, 'c3', 'entity_search:123', '');",
        )
        .expect("seed legacy");
        drop(conn);

        let conn = open_index(&root).expect("open index and migrate");
        let chunk_rows: Vec<(i64, Option<String>, Option<String>)> = {
            let mut stmt = conn
                .prepare("SELECT rowid, path, stream FROM chunks ORDER BY rowid")
                .expect("prepare");
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect")
        };
        assert_eq!(
            chunk_rows,
            vec![
                (1, Some("p1".to_string()), Some("s1".to_string())),
                (2, None, None),
                (
                    3,
                    Some("entity_search:123".to_string()),
                    Some("".to_string())
                ),
            ]
        );
        let ready = chunk_path_lookup_ready(&conn).expect("read readiness");
        assert!(!ready);
        drop(conn);

        let status_before = inspect_path_lookup(&root).expect("inspect before");
        assert!(!status_before.ready);

        let status_after = apply_path_lookup(&root).expect("apply");
        assert!(status_after.ready);

        let conn = Connection::open(db_path(&root)).expect("open repaired");
        let rows = {
            let mut stmt = conn
                .prepare("SELECT rowid, path, stream FROM chunk_sources ORDER BY rowid")
                .expect("prepare");
            stmt.query_map([], |row| {
                Ok((
                    row.get::<_, i64>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<String>>(2)?,
                ))
            })
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect")
        };
        assert_eq!(
            rows,
            vec![
                (1, Some("p1".to_string()), Some("s1".to_string())),
                (2, None, None),
                (
                    3,
                    Some("entity_search:123".to_string()),
                    Some("".to_string())
                ),
            ]
        );
        drop(conn);

        let status_final = inspect_path_lookup(&root).expect("inspect final");
        assert!(status_final.ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn validation_detects_swapped_path_stream_missing_and_extra_rows() {
        let root = temp_root("validation-cases");
        let conn = open_index(&root).expect("open");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES (1, 'c1', 'p1', 's1'), (2, 'c2', 'p2', 's2')",
            [],
        )
        .expect("seed");
        drop(conn);
        apply_path_lookup(&root).expect("apply");

        let conn = Connection::open(db_path(&root)).expect("open for tampering");
        // Case 1: Swapped path/stream with equal count
        conn.execute("UPDATE chunk_sources SET stream = 's2' WHERE rowid = 1", [])
            .expect("swap");
        let diff: Option<i64> = conn
            .query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r.get(0))
            .optional()
            .expect("check diff");
        assert!(diff.is_some());
        drop(conn);
        assert!(inspect_path_lookup(&root).expect("inspect").ready);

        // Re-apply to repair
        apply_path_lookup(&root).expect("re-apply");
        let conn = Connection::open(db_path(&root)).expect("open");
        let diff: Option<i64> = conn
            .query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r.get(0))
            .optional()
            .expect("check diff");
        assert!(diff.is_none());

        // Case 2: Missing row in chunk_sources
        conn.execute("DELETE FROM chunk_sources WHERE rowid = 1", [])
            .expect("delete");
        let diff: Option<i64> = conn
            .query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r.get(0))
            .optional()
            .expect("check diff");
        assert!(diff.is_some());
        drop(conn);
        assert!(inspect_path_lookup(&root).expect("inspect").ready);

        // Case 3: Extra row in chunk_sources
        apply_path_lookup(&root).expect("re-apply");
        let conn = Connection::open(db_path(&root)).expect("open");
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES (99, 'extra', 'extra')",
            [],
        )
        .expect("insert extra");
        let diff: Option<i64> = conn
            .query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r.get(0))
            .optional()
            .expect("check diff");
        assert!(diff.is_some());
        drop(conn);
        assert!(inspect_path_lookup(&root).expect("inspect").ready);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn older_writer_drift_and_repair() {
        let root = temp_root("older-writer-drift");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES (1, 'c1', 'p1', 's1')",
            [],
        )
        .expect("seed");
        drop(conn);
        apply_path_lookup(&root).expect("initial apply");
        assert!(inspect_path_lookup(&root).expect("inspect").ready);

        // Direct write to chunks simulating older writer
        let conn = Connection::open(db_path(&root)).expect("open");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES (2, 'c2', 'p2', 's2')",
            [],
        )
        .expect("drift insert");
        drop(conn);
        // Inspect reads marker only, so it still reports ready: true
        assert!(inspect_path_lookup(&root).expect("inspect").ready);

        // Install abort trigger on chunk_source_readiness so apply fails
        let conn = Connection::open(db_path(&root)).expect("open");
        conn.execute(
            "CREATE TRIGGER abort_on_readiness BEFORE INSERT ON chunk_source_readiness BEGIN SELECT RAISE(ABORT, 'cannot update readiness'); END;",
            [],
        )
        .expect("create trigger");
        drop(conn);

        let err = apply_path_lookup(&root).unwrap_err();
        assert!(matches!(err, StoreError::Sql(_)));

        // Check drifted row is still present in FTS, chunk_sources has no rowid 2, and marker stays ready
        let conn = Connection::open(db_path(&root)).expect("open");
        let chunk_count: i64 = conn
            .query_row("SELECT count(*) FROM chunks WHERE rowid = 2", [], |r| {
                r.get(0)
            })
            .expect("count chunks");
        assert_eq!(chunk_count, 1);
        let cs_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunk_sources WHERE rowid = 2",
                [],
                |r| r.get(0),
            )
            .expect("count chunk_sources");
        assert_eq!(cs_count, 0);
        let ready_val: i64 = conn
            .query_row(
                "SELECT ready FROM chunk_source_readiness WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .expect("readiness");
        assert_eq!(ready_val, 1);

        // Drop trigger and apply repairs
        conn.execute("DROP TRIGGER abort_on_readiness", [])
            .expect("drop trigger");
        drop(conn);

        let status = apply_path_lookup(&root).expect("repair apply");
        assert!(status.ready);

        let conn = Connection::open(db_path(&root)).expect("open");
        let cs_count: i64 = conn
            .query_row(
                "SELECT count(*) FROM chunk_sources WHERE rowid = 2 AND path = 'p2' AND stream = 's2'",
                [],
                |r| r.get(0),
            )
            .expect("count chunk_sources");
        assert_eq!(cs_count, 1);

        let mut stmt = conn
            .prepare(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE)
            .expect("prepare validation");
        let invalid: Vec<i64> = stmt
            .query_map([], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert!(invalid.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn apply_path_lookup_rollback_on_failure() {
        let root = temp_root("rollback-on-failure");
        let conn = open_index(&root).expect("open");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES (1, 'c1', 'p1', 's1')",
            [],
        )
        .expect("seed");
        conn.execute(
            "UPDATE chunk_source_readiness SET ready = 0 WHERE id = 1",
            [],
        )
        .expect("mark unready");
        conn.execute(
            "CREATE TRIGGER abort_on_chunk_sources BEFORE INSERT ON chunk_sources BEGIN SELECT RAISE(ABORT, 'boom'); END;",
            [],
        )
        .expect("trigger");
        drop(conn);

        let result = apply_path_lookup(&root);
        assert!(result.is_err());

        let conn = Connection::open(db_path(&root)).expect("open");
        let chunk: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT rowid, path, stream FROM chunks WHERE rowid = 1",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("read chunk");
        assert_eq!(chunk, (1, "p1".to_string(), Some("s1".to_string())));

        let cs_count: i64 = conn
            .query_row("SELECT count(*) FROM chunk_sources", [], |r| r.get(0))
            .expect("count chunk_sources");
        assert_eq!(cs_count, 0);

        let readiness: Option<i64> = conn
            .query_row(
                "SELECT ready FROM chunk_source_readiness WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .optional()
            .expect("read readiness");
        assert_eq!(readiness, Some(0));
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn seed_rejects_equal_count_corruption_and_rolls_back_readiness_and_rows() {
        let root = temp_root("seed-correspondence-rollback");
        let conn = open_index(&root).unwrap();
        conn.execute_batch(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES
             (1, 'source one', 'first.md', 'stream-one'), (2, 'source two', NULL, NULL);
             INSERT INTO chunk_sources(rowid, path, stream) VALUES
             (1, 'first.md', 'stream-one'), (2, NULL, NULL);
             UPDATE chunk_source_readiness SET ready=0 WHERE id=1;
             CREATE TRIGGER corrupt_seed AFTER INSERT ON chunk_sources BEGIN
               UPDATE chunk_sources SET stream='corrupt' WHERE rowid=NEW.rowid;
             END;",
        )
        .unwrap();
        drop(conn);
        let error = apply_path_lookup(&root).unwrap_err();
        assert!(error.to_string().contains("does not correspond"), "{error}");
        assert!(!inspect_path_lookup(&root).unwrap().ready);
        let conn = Connection::open(db_path(&root)).unwrap();
        let rows: Vec<(i64, Option<String>, Option<String>)> = {
            let mut statement = conn
                .prepare("SELECT rowid, path, stream FROM chunk_sources ORDER BY rowid")
                .unwrap();
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
                .unwrap()
                .collect::<Result<_, _>>()
                .unwrap()
        };
        assert_eq!(
            rows,
            vec![
                (
                    1,
                    Some("first.md".to_owned()),
                    Some("stream-one".to_owned())
                ),
                (2, None, None),
            ]
        );
        assert_eq!(
            conn.query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r
                .get::<_, i64>(0))
                .optional()
                .unwrap(),
            None
        );
        conn.execute("DROP TRIGGER corrupt_seed", []).unwrap();
        drop(conn);
        assert!(apply_path_lookup(&root).unwrap().ready);
        let conn = Connection::open(db_path(&root)).unwrap();
        assert_eq!(
            conn.query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r
                .get::<_, i64>(0))
                .optional()
                .unwrap(),
            None
        );
        drop(conn);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn reset_index_leaves_ready_empty_mapping_and_scan_works() {
        let root = temp_root("reset-and-scan");
        let path = root.join("chronicle/20260101/stream-a/20260101T120000Z/transcript.jsonl");
        fs::create_dir_all(path.parent().unwrap()).expect("create chronicle dir");
        fs::write(&path, r#"{"text":"sample content"}"#).expect("write file");

        reset_index(&root).expect("reset index");
        let conn = Connection::open(db_path(&root)).expect("open");
        assert!(chunk_path_lookup_ready(&conn).expect("ready"));
        let count: i64 = conn
            .query_row("SELECT count(*) FROM chunk_sources", [], |r| r.get(0))
            .expect("count");
        assert_eq!(count, 0);
        drop(conn);

        scan_journal(&root, false).expect("light scan");
        scan_journal(&root, true).expect("full scan");
        let status = inspect_path_lookup(&root).expect("inspect");
        assert!(status.ready);
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn unready_index_seeds_on_first_admitted_write() {
        // An index built before the path lookup existed keeps indexing after
        // upgrade: the first scan seeds the lookup, and later writes use it.
        let scan_root = temp_root("unready-scan-seeds");
        let file_path = scan_root.join("facets/work/events/20260101.jsonl");
        fs::create_dir_all(file_path.parent().unwrap()).expect("create dir");
        fs::write(&file_path, r#"{"type":"meeting","title":"Standup"}"#).expect("write file");
        let conn = open_index(&scan_root).expect("open index");
        conn.execute(
            "UPDATE chunk_source_readiness SET ready = 0 WHERE id = 1",
            [],
        )
        .expect("set unready");
        drop(conn);
        assert!(!inspect_path_lookup(&scan_root).expect("inspect").ready);

        let report = scan_journal(&scan_root, false).expect("scan seeds lookup");
        assert_eq!(report.indexed, 1);
        assert!(inspect_path_lookup(&scan_root).expect("inspect").ready);
        fs::write(&file_path, r#"{"type":"meeting","title":"Retro"}"#).expect("rewrite file");
        rescan_file(&scan_root, &file_path).expect("rescan uses lookup");
        let conn = Connection::open(db_path(&scan_root)).expect("open");
        let rows: Vec<(String, String)> = {
            let mut stmt = conn
                .prepare("SELECT c.path, c.content FROM chunks AS c JOIN chunk_sources AS s ON s.rowid = c.rowid")
                .expect("prepare");
            stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect")
        };
        let total: i64 = conn
            .query_row("SELECT count(*) FROM chunks", [], |r| r.get(0))
            .expect("count");
        assert_eq!(total, rows.len() as i64);
        assert!(
            rows.iter()
                .all(|(path, _)| path == "facets/work/events/20260101.jsonl")
        );
        assert!(rows.iter().any(|(_, content)| content.contains("Retro")));
        assert!(!rows.iter().any(|(_, content)| content.contains("Standup")));
        drop(conn);
        let _ = fs::remove_dir_all(&scan_root);

        let root = temp_root("unready-prune-seeds");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "UPDATE chunk_source_readiness SET ready = 0 WHERE id = 1",
            [],
        )
        .expect("set unready");
        drop(conn);

        // Seed data for prunes on unready database, including survivor rowid 4
        let conn = Connection::open(db_path(&root)).expect("open");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES
             (1, 'stream chunk', 'chronicle/20260101/stream-a/seg/transcript.jsonl', 'stream-a'),
             (2, 'path chunk', 'facets/work/events/20260101.jsonl', 'stream-b'),
             (3, 'chat chunk', '20260101/chat/123456_300/chat.jsonl', 'stream-c'),
             (4, 'other chunk', 'chronicle/20260102/other/seg/transcript.jsonl', 'other-stream')",
            [],
        )
        .expect("insert chunks");
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES
             (1, 'chronicle/20260101/stream-a/seg/transcript.jsonl', 'stream-a'),
             (2, 'facets/work/events/20260101.jsonl', 'stream-b'),
             (3, '20260101/chat/123456_300/chat.jsonl', 'stream-c'),
             (4, 'chronicle/20260102/other/seg/transcript.jsonl', 'other-stream')",
            [],
        )
        .expect("insert chunk_sources");
        drop(conn);

        // Prunes on an unready database seed the lookup first, then prune both tables
        let counts_stream = prune_chunks_by_stream(&root, "stream-a").expect("prune stream");
        assert_eq!(counts_stream.chunks, 1);

        // Assert survivor (4) and not-yet-pruned (2, 3) are still in both tables, and rowid 1 is gone from both
        let conn = Connection::open(db_path(&root)).expect("open");
        let chunk_rowids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM chunks ORDER BY rowid ASC")
                .expect("prepare chunks");
            stmt.query_map([], |r| r.get(0))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect")
        };
        assert_eq!(chunk_rowids, vec![2, 3, 4]);

        let source_rowids: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM chunk_sources ORDER BY rowid ASC")
                .expect("prepare sources");
            stmt.query_map([], |r| r.get(0))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect")
        };
        assert_eq!(source_rowids, vec![2, 3, 4]);
        drop(conn);

        let counts_paths =
            prune_by_paths(&root, &["facets/work/events/20260101.jsonl"]).expect("prune paths");
        assert!(counts_paths.is_some());
        assert_eq!(counts_paths.unwrap().chunks, 1);

        let counts_chat = prune_authored_chat_paths(&root).expect("prune chat");
        assert!(counts_chat.is_some());
        assert_eq!(counts_chat.unwrap().chunks, 1);

        // Verify only the survivor remains in both tables and the lookup is ready
        let conn = Connection::open(db_path(&root)).expect("open");
        let remaining_chunks: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM chunks ORDER BY rowid ASC")
                .expect("prepare chunks");
            stmt.query_map([], |r| r.get(0))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect")
        };
        assert_eq!(remaining_chunks, vec![4]);

        let remaining_sources: Vec<i64> = {
            let mut stmt = conn
                .prepare("SELECT rowid FROM chunk_sources ORDER BY rowid ASC")
                .expect("prepare sources");
            stmt.query_map([], |r| r.get(0))
                .expect("query")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect")
        };
        assert_eq!(remaining_sources, vec![4]);

        let ready_val: i64 = conn
            .query_row(
                "SELECT ready FROM chunk_source_readiness WHERE id = 1",
                [],
                |r| r.get(0),
            )
            .expect("ready");
        assert_eq!(ready_val, 1);
        drop(conn);

        // Legacy database without chunk_sources table: prune_authored_chat_paths succeeds and does not create chunk_sources
        let legacy_root = temp_root("legacy-no-chunk-sources");
        let legacy_index_dir = legacy_root.join("indexer");
        fs::create_dir_all(&legacy_index_dir).expect("create dir");
        let conn = Connection::open(db_path(&legacy_root)).expect("open legacy");
        conn.execute_batch(
            "CREATE VIRTUAL TABLE chunks USING fts5(
                content,
                path UNINDEXED,
                day UNINDEXED,
                facet UNINDEXED,
                agent UNINDEXED,
                stream UNINDEXED,
                idx UNINDEXED,
                time_bucket UNINDEXED
            );
            INSERT INTO chunks(rowid, content, path, stream) VALUES
             (1, 'chat chunk', '20260101/chat/123456_300/chat.jsonl', 'stream-c');",
        )
        .expect("seed legacy chunks");
        drop(conn);

        let chat_res = prune_authored_chat_paths(&legacy_root).expect("prune chat on legacy");
        assert!(chat_res.is_some());
        assert_eq!(chat_res.unwrap().chunks, 1);

        let conn = Connection::open(db_path(&legacy_root)).expect("open legacy check");
        let table_exists: bool = conn
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='chunk_sources')",
                [],
                |r| r.get(0),
            )
            .expect("check table exists");
        assert!(!table_exists);
        let _ = fs::remove_dir_all(legacy_root);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn explain_query_plan_verifies_index_usage() {
        let root = temp_root("explain-plan");
        let conn = open_index(&root).expect("open index");

        let plan1: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {CHUNK_SOURCES_LOOKUP_ROWIDS}"))
                .expect("prepare explain 1");
            stmt.query_map(["path"], |row| row.get::<_, String>(3))
                .expect("query explain 1")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect 1")
        };
        assert!(
            plan1
                .iter()
                .any(|p| p.contains("USING INDEX chunk_sources_path")
                    || p.contains("USING COVERING INDEX chunk_sources_path")),
            "Plan 1 must use index: {:?}",
            plan1
        );
        assert!(
            !plan1
                .iter()
                .any(|p| p.to_lowercase().contains("scan chunks"))
        );

        let plan2: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {CHUNK_SOURCES_LOOKUP_STREAM}"))
                .expect("prepare explain 2");
            stmt.query_map(["path"], |row| row.get::<_, String>(3))
                .expect("query explain 2")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect 2")
        };
        assert!(
            plan2
                .iter()
                .any(|p| p.contains("USING INDEX chunk_sources_path")
                    || p.contains("USING COVERING INDEX chunk_sources_path")),
            "Plan 2 must use index: {:?}",
            plan2
        );
        assert!(
            !plan2
                .iter()
                .any(|p| p.to_lowercase().contains("scan chunks"))
        );

        let plan3: Vec<String> = {
            let mut stmt = conn
                .prepare(&format!("EXPLAIN QUERY PLAN {CHUNK_SOURCES_LOOKUP_PATHS}"))
                .expect("prepare explain 3");
            stmt.query_map(params!["path", 10], |row| row.get::<_, String>(3))
                .expect("query explain 3")
                .collect::<Result<Vec<_>, _>>()
                .expect("collect 3")
        };
        assert!(
            plan3
                .iter()
                .any(|p| p.contains("USING INDEX chunk_sources_path")
                    || p.contains("USING COVERING INDEX chunk_sources_path")),
            "Plan 3 must use index: {:?}",
            plan3
        );
        assert!(
            !plan3
                .iter()
                .any(|p| p.to_lowercase().contains("scan chunks"))
        );

        // Test querying known and absent paths
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES (1, 'known/path.md', 'stream1'), (2, 'known/path.md', 'stream1')",
            [],
        )
        .expect("insert chunk_sources");

        let mut stmt = conn
            .prepare(CHUNK_SOURCES_LOOKUP_ROWIDS)
            .expect("prepare lookup");
        let known_rowids: Vec<i64> = stmt
            .query_map(["known/path.md"], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert_eq!(known_rowids, vec![1, 2]);

        let absent_rowids: Vec<i64> = stmt
            .query_map(["absent/path.md"], |r| r.get(0))
            .expect("query")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect");
        assert!(absent_rowids.is_empty());

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn read_only_match_is_unaffected_by_readiness() {
        let root = temp_root("read-only-match");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path) VALUES (1, 'hello world', 'p1')",
            [],
        )
        .expect("seed");
        conn.execute(
            "UPDATE chunk_source_readiness SET ready = 0 WHERE id = 1",
            [],
        )
        .expect("unready");
        let sql_before: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='chunks'",
                [],
                |row| row.get(0),
            )
            .expect("query sql before");
        drop(conn);

        let db_file = db_path(&root);
        let size_before = fs::metadata(&db_file).expect("metadata before").len();

        let status = inspect_path_lookup(&root).expect("inspect");
        assert!(!status.ready);

        let size_after = fs::metadata(&db_file).expect("metadata after").len();
        assert_eq!(size_before, size_after);

        let conn = Connection::open_with_flags(&db_file, OpenFlags::SQLITE_OPEN_READ_ONLY)
            .expect("open ro");
        let found: i64 = conn
            .query_row(
                "SELECT rowid FROM chunks WHERE chunks MATCH 'hello'",
                [],
                |row| row.get(0),
            )
            .expect("match query");
        assert_eq!(found, 1);

        let sql_after: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE name='chunks'",
                [],
                |row| row.get(0),
            )
            .expect("query sql after");
        assert_eq!(sql_before, sql_after);

        let ready_val: i64 = conn
            .query_row(
                "SELECT ready FROM chunk_source_readiness WHERE id = 1",
                [],
                |row| row.get(0),
            )
            .expect("readiness");
        assert_eq!(ready_val, 0);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn same_path_stream_lookup_uses_lowest_rowid_for_classification() {
        let root = temp_root("same-path-stream");
        let conn = open_index(&root).expect("open index");
        let path = "facets/Work/news/20260107.md";
        let declarations = FacetDeclarationSet::default();

        // Lowest rowid stream SQL NULL, later rowid stream mcp.agent
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES
             (1, ?1, NULL),
             (2, ?1, 'mcp.agent')",
            [path],
        )
        .expect("insert chunk_sources case 1");

        let stream: Option<String> = conn
            .query_row(CHUNK_SOURCES_LOOKUP_STREAM, [path], |row| row.get(0))
            .expect("query stream case 1");
        assert_eq!(stream, None);

        let classification = classify_source(&root, path, stream.as_deref(), &declarations);
        assert!(classification.eligible);

        // Reversed rowids: lowest rowid mcp.agent, later rowid NULL
        conn.execute("DELETE FROM chunk_sources WHERE path = ?", [path])
            .expect("delete chunk_sources");
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES
             (1, ?1, 'mcp.agent'),
             (2, ?1, NULL)",
            [path],
        )
        .expect("insert chunk_sources case 2");

        let stream_rev: Option<String> = conn
            .query_row(CHUNK_SOURCES_LOOKUP_STREAM, [path], |row| row.get(0))
            .expect("query stream case 2");
        assert_eq!(stream_rev, Some("mcp.agent".to_string()));

        let classification_rev = classify_source(&root, path, stream_rev.as_deref(), &declarations);
        assert!(!classification_rev.eligible);

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn next_unclassified_chunk_paths_excludes_null_and_empty_paths() {
        let root = temp_root("next-unclassified-paths");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES
             (1, NULL, 'stream1'),
             (2, '', 'stream2'),
             (3, 'valid/path/1.md', 'stream3'),
             (4, 'valid/path/2.md', 'stream4')",
            [],
        )
        .expect("insert chunk_sources");

        let paths = next_unclassified_chunk_paths(&conn, "", 10).expect("next unclassified");
        assert_eq!(
            paths,
            vec!["valid/path/1.md".to_string(), "valid/path/2.md".to_string()]
        );
        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn abort_trigger_during_file_replacement_preserves_unrelated_path() {
        let root = temp_root("abort-trigger-preserves-unrelated");
        let file1 = root.join("facets/work/events/20260101.jsonl");
        let file2 = root.join("facets/work/events/20260102.jsonl");
        fs::create_dir_all(file1.parent().unwrap()).expect("create dir");
        fs::write(&file1, r#"{"type":"meeting","title":"Standup 1"}"#).expect("write file 1");
        fs::write(&file2, r#"{"type":"meeting","title":"Standup 2"}"#).expect("write file 2");

        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(rowid, content, path, stream) VALUES
             (1, 'content 1', 'facets/work/events/20260101.jsonl', 'stream1'),
             (2, 'content 2', 'facets/work/events/20260102.jsonl', 'stream2')",
            [],
        )
        .expect("insert chunks");
        conn.execute(
            "INSERT INTO chunk_sources(rowid, path, stream) VALUES
             (1, 'facets/work/events/20260101.jsonl', 'stream1'),
             (2, 'facets/work/events/20260102.jsonl', 'stream2')",
            [],
        )
        .expect("insert chunk_sources");
        conn.execute(
            "CREATE TRIGGER abort_on_chunk_sources_insert BEFORE INSERT ON chunk_sources BEGIN SELECT RAISE(ABORT, 'chunk_sources insert blocked'); END;",
            [],
        )
        .expect("trigger");
        drop(conn);

        let err = rescan_file(&root, &file1).unwrap_err();
        assert!(matches!(err, StoreError::Io(_) | StoreError::Sql(_)));
        assert!(
            err.to_string().contains("chunk_sources insert blocked"),
            "{err}"
        );

        let conn = Connection::open(db_path(&root)).expect("open");
        let target_before_retry: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT rowid, content, stream FROM chunks WHERE path = ?",
                ["facets/work/events/20260101.jsonl"],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("target chunk survives rollback");
        assert_eq!(
            target_before_retry,
            (1, "content 1".to_owned(), Some("stream1".to_owned()))
        );
        let target_mapping: (i64, Option<String>) = conn
            .query_row(
                "SELECT rowid, stream FROM chunk_sources WHERE path = ?",
                ["facets/work/events/20260101.jsonl"],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .expect("target mapping survives rollback");
        assert_eq!(target_mapping, (1, Some("stream1".to_owned())));
        let chunk2: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT rowid, content, stream FROM chunks WHERE path = 'facets/work/events/20260102.jsonl'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("query chunk2");
        assert_eq!(
            chunk2,
            (2, "content 2".to_string(), Some("stream2".to_string()))
        );

        let cs2: (i64, String, Option<String>) = conn
            .query_row(
                "SELECT rowid, path, stream FROM chunk_sources WHERE path = 'facets/work/events/20260102.jsonl'",
                [],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .expect("query cs2");
        assert_eq!(
            cs2,
            (
                2,
                "facets/work/events/20260102.jsonl".to_string(),
                Some("stream2".to_string())
            )
        );
        conn.execute("DROP TRIGGER abort_on_chunk_sources_insert", [])
            .unwrap();
        drop(conn);
        assert!(matches!(
            rescan_file(&root, &file1).unwrap(),
            crate::scan::RescanFileStatus::Indexed { .. }
        ));
        let conn = Connection::open(db_path(&root)).unwrap();
        let target_text: String = conn
            .query_row(
                "SELECT content FROM chunks WHERE path = ?",
                ["facets/work/events/20260101.jsonl"],
                |r| r.get(0),
            )
            .unwrap();
        assert!(target_text.contains("Standup 1"));
        let difference: Option<i64> = conn
            .query_row(VALIDATE_CHUNK_SOURCES_CORRESPONDENCE, [], |r| r.get(0))
            .optional()
            .unwrap();
        assert_eq!(
            difference, None,
            "successful retry restores exact paired correspondence"
        );

        let _ = fs::remove_dir_all(root);
    }

    #[test]
    fn rebuild_edges_and_reconcile_stale_classifications_on_unready_index_seed_lookup() {
        let root = temp_root("unready-rebuild-reconcile");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "UPDATE chunk_source_readiness SET ready = 0 WHERE id = 1",
            [],
        )
        .expect("set unready");
        drop(conn);

        let edge_report = rebuild_edges(&root).expect("rebuild edges");
        assert_eq!(edge_report.failed, 0);

        let mut snapshot = || FacetDeclarationSet::from_journal(&root);
        let reconcile_report =
            reconcile_stale_classifications(&root, &mut snapshot).expect("reconcile");
        assert!(!reconcile_report.incomplete);

        let status = inspect_path_lookup(&root).expect("inspect");
        assert!(status.ready);

        let _ = fs::remove_dir_all(root);
    }
}
