// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags, OptionalExtension, Transaction, params};

use crate::StoreError;

pub const INDEX_DIR: &str = "indexer";
pub const DB_NAME: &str = "journal.sqlite";

/// Counts removed by an index cleanup.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct StreamPruneCounts {
    pub chunks: u64,
    pub files: u64,
}

const CREATE_FILES: &str = "CREATE TABLE IF NOT EXISTS files(path TEXT PRIMARY KEY, mtime INTEGER)";
const CREATE_CHUNKS: &str = "\
CREATE VIRTUAL TABLE IF NOT EXISTS chunks USING fts5(
content,
path UNINDEXED,
day UNINDEXED,
facet UNINDEXED,
agent UNINDEXED,
stream UNINDEXED,
idx UNINDEXED,
time_bucket UNINDEXED
)";
// Rust is the production schema authority. The Python constants remain only as
// a differential reference while the rest of the Python tree is converted.
pub(crate) const EDGES_SCHEMA_PATH: &str = "edges:__schema__";
pub(crate) const EDGES_SCHEMA_VERSION: i64 = 1;
pub(crate) const INDEX_BUILD_STATE_SCHEMA_VERSION: i64 = 1;
const CREATE_EDGE_FILES: &str =
    "CREATE TABLE IF NOT EXISTS edge_files(path TEXT PRIMARY KEY, mtime INTEGER)";
const CREATE_EDGES: &str = "\
CREATE TABLE IF NOT EXISTS edges(
src TEXT NOT NULL,
dst TEXT NOT NULL,
kind TEXT NOT NULL,
directed INTEGER NOT NULL,
src_name TEXT,
dst_name TEXT,
day TEXT,
facet TEXT,
source TEXT NOT NULL,
path TEXT NOT NULL,
anchor TEXT,
label TEXT,
ts INTEGER,
weight INTEGER NOT NULL
)";
const CREATE_EDGES_PATH_INDEX: &str = "CREATE INDEX IF NOT EXISTS edges_path ON edges(path)";
const CREATE_EDGES_SRC_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_edges_src ON edges(src, kind, day)";
const CREATE_EDGES_DST_INDEX: &str =
    "CREATE INDEX IF NOT EXISTS idx_edges_dst ON edges(dst, kind, day)";
const CREATE_INDEX_BUILD_STATE: &str = "\
CREATE TABLE IF NOT EXISTS index_build_state(
id INTEGER PRIMARY KEY CHECK (id = 1),
schema_version INTEGER NOT NULL,
state TEXT NOT NULL CHECK (state IN ('building', 'complete')),
files_count INTEGER NOT NULL CHECK (files_count >= 0),
chunks_count INTEGER NOT NULL CHECK (chunks_count >= 0)
)";
const CREATE_ENTITY_SEARCH_WATERMARK: &str = "\
CREATE TABLE IF NOT EXISTS entity_search_watermark(
id INTEGER PRIMARY KEY CHECK (id = 1),
mtime INTEGER NOT NULL,
count INTEGER NOT NULL
)";
const CREATE_SEGMENT_AGGREGATE_MIGRATION: &str = "\
CREATE TABLE IF NOT EXISTS segment_aggregate_migration(
id INTEGER PRIMARY KEY CHECK (id = 1),
cursor TEXT NOT NULL,
completed INTEGER NOT NULL CHECK (completed IN (0, 1))
)";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum IndexBuildLifecycle {
    Building,
    Complete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct IndexBuildState {
    pub schema_version: i64,
    pub state: IndexBuildLifecycle,
    pub files_count: i64,
    pub chunks_count: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct EntitySearchWatermark {
    pub mtime: i64,
    pub count: i64,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SegmentAggregateMigration {
    pub cursor: String,
    pub completed: bool,
}

pub fn db_path(journal: &Path) -> PathBuf {
    journal.join(INDEX_DIR).join(DB_NAME)
}

/// Whether an existing journal index can be opened without mutating it.
pub fn is_index_readable(journal: &Path) -> bool {
    let path = db_path(journal);
    path.is_file() && Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).is_ok()
}

pub fn open_index(journal: &Path) -> Result<Connection, StoreError> {
    let index_dir = journal.join(INDEX_DIR);
    fs::create_dir_all(&index_dir)?;
    let mut conn = Connection::open(db_path(journal))?;
    conn.execute_batch(
        "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    )?;
    ensure_schema(&mut conn)?;
    Ok(conn)
}

pub fn reset_index(journal: &Path) -> Result<(), StoreError> {
    let index_dir = journal.join(INDEX_DIR);
    fs::create_dir_all(&index_dir)?;
    let mut conn = Connection::open(db_path(journal))?;
    conn.execute_batch(
        "PRAGMA busy_timeout=5000; PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL;",
    )?;
    let tx = conn.transaction()?;

    if sqlite_table_exists(&tx, "chunks")? {
        tx.execute("DROP TABLE chunks", [])?;
    } else {
        for table in [
            "chunks_config",
            "chunks_docsize",
            "chunks_content",
            "chunks_idx",
            "chunks_data",
        ] {
            tx.execute(&format!("DROP TABLE IF EXISTS {table}"), [])?;
        }
    }
    tx.execute("DROP INDEX IF EXISTS edges_path", [])?;
    tx.execute("DROP INDEX IF EXISTS idx_edges_src", [])?;
    tx.execute("DROP INDEX IF EXISTS idx_edges_dst", [])?;
    tx.execute("DROP TABLE IF EXISTS edges", [])?;
    tx.execute("DROP TABLE IF EXISTS edge_files", [])?;
    tx.execute("DROP TABLE IF EXISTS files", [])?;
    create_schema(&tx)?;
    tx.execute("DELETE FROM entity_search_watermark", [])?;
    tx.execute("DELETE FROM segment_aggregate_migration", [])?;
    tx.execute(
        "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, ?, 'building', 0, 0)",
        [INDEX_BUILD_STATE_SCHEMA_VERSION],
    )?;
    tx.commit()?;
    Ok(())
}

/// Remove indexed chunks for one stream and their corresponding file rows.
pub fn prune_chunks_by_stream(
    journal: &Path,
    stream: &str,
) -> Result<StreamPruneCounts, StoreError> {
    let mut conn = open_index(journal)?;
    let tx = conn.transaction()?;
    let paths = {
        let mut statement = tx.prepare("SELECT DISTINCT path FROM chunks WHERE stream=?")?;
        statement
            .query_map([stream], |row| row.get::<_, String>(0))?
            .collect::<Result<Vec<_>, _>>()?
    };
    let chunks = tx.execute("DELETE FROM chunks WHERE stream=?", [stream])? as u64;
    let mut files = 0;
    for path in paths {
        files += tx.execute("DELETE FROM files WHERE path=?", [path])? as u64;
    }
    tx.commit()?;
    Ok(StreamPruneCounts { chunks, files })
}

/// Drop index rows for paths that have already been removed from the chronicle.
///
/// Each `rel` is matched exactly **and** as a directory prefix, so passing a
/// segment's path clears the segment and everything that was inside it. Missing
/// rows are not an error: the caller's authority is the filesystem, and being
/// told about a path this index never held is ordinary.
///
/// ⛔ **Never call this before the paths are actually gone.** The index is a
/// derived cache that re-converges on the chronicle every scan, so the two
/// orderings fail differently and only one of them fails safely: remove-then-tell
/// leaves rows the next scan deletes, because the file is gone — a loud, local
/// failure on a code path that runs. Tell-then-remove leaves files on disk the
/// index does not list, which is silently invisible owner data, indistinguishable
/// from misremembering, surviving until someone runs a full rebuild.
///
/// ⚠ This is the inverse of the ordering a content-addressed store uses, and the
/// difference is which side is authoritative: there the index is, and the blobs are
/// derived. Here the chronicle is authoritative and the index is the cache.
///
/// Returns `None` when the journal has no index. ⛔ Deliberately not
/// [`open_index`], which creates the database: a prune must never be the thing
/// that brings an index into existence.
pub fn prune_by_paths(
    journal: &Path,
    rels: &[&str],
) -> Result<Option<StreamPruneCounts>, StoreError> {
    if !db_path(journal).exists() {
        return Ok(None);
    }
    let mut conn = open_index(journal)?;
    let tx = conn.transaction()?;
    let mut counts = StreamPruneCounts::default();
    for rel in rels {
        let prefix = format!("{rel}/%");
        counts.chunks += tx.execute(
            "DELETE FROM chunks WHERE path=?1 OR path LIKE ?2",
            rusqlite::params![rel, &prefix],
        )? as u64;
        counts.files += tx.execute(
            "DELETE FROM files WHERE path=?1 OR path LIKE ?2",
            rusqlite::params![rel, &prefix],
        )? as u64;
    }
    tx.commit()?;
    Ok(Some(counts))
}

/// SQL predicate for journal-authored chat rows, with either indexed path shape.
/// Readers exclude these rows; the index writer owns their removal.
pub const AUTHORED_CHAT_PATH_PREDICATE: &str =
    "path LIKE '________/chat/%/chat.jsonl' OR path LIKE 'chronicle/________/chat/%/chat.jsonl'";

/// Drop index rows for journal-authored `YYYYMMDD/chat/<segment>/chat.jsonl` paths.
///
/// Also matches the optional `chronicle/` prefix. Returns `None` when the journal
/// has no index and does not create one.
pub fn prune_authored_chat_paths(journal: &Path) -> Result<Option<StreamPruneCounts>, StoreError> {
    let path = db_path(journal);
    if !path.is_file() {
        return Ok(None);
    }
    let mut conn = Connection::open(&path)?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    let tx = conn.transaction()?;
    let chunks = tx.execute(
        &format!("DELETE FROM chunks WHERE {AUTHORED_CHAT_PATH_PREDICATE}"),
        [],
    )? as u64;
    let files = tx.execute(
        &format!("DELETE FROM files WHERE {AUTHORED_CHAT_PATH_PREDICATE}"),
        [],
    )? as u64;
    tx.commit()?;
    Ok(Some(StreamPruneCounts { chunks, files }))
}

fn ensure_schema(conn: &mut Connection) -> Result<(), StoreError> {
    let tx = conn.transaction()?;
    migrate_legacy_chunks(&tx)?;
    create_schema(&tx)?;
    tx.commit()?;
    Ok(())
}

fn migrate_legacy_chunks(conn: &Connection) -> Result<(), StoreError> {
    if !sqlite_table_exists(conn, "chunks")? {
        return Ok(());
    }
    let columns = {
        let mut statement = conn.prepare("PRAGMA table_info(chunks)")?;
        statement
            .query_map([], |row| row.get::<_, String>(1))?
            .collect::<Result<Vec<_>, _>>()?
    };
    if columns.iter().any(|column| column == "time_bucket") {
        return Ok(());
    }

    conn.execute("DROP TABLE IF EXISTS chunks_native_migration", [])?;
    conn.execute(
        "CREATE VIRTUAL TABLE chunks_native_migration USING fts5(
            content,
            path UNINDEXED,
            day UNINDEXED,
            facet UNINDEXED,
            agent UNINDEXED,
            stream UNINDEXED,
            idx UNINDEXED,
            time_bucket UNINDEXED
        )",
        [],
    )?;
    let stream = if columns.iter().any(|column| column == "stream") {
        "stream"
    } else {
        "''"
    };
    conn.execute(
        &format!(
            "INSERT INTO chunks_native_migration(content,path,day,facet,agent,stream,idx,time_bucket) \
             SELECT content,path,day,facet,agent,{stream},idx,'' FROM chunks"
        ),
        [],
    )?;
    conn.execute("DROP TABLE chunks", [])?;
    conn.execute("ALTER TABLE chunks_native_migration RENAME TO chunks", [])?;
    Ok(())
}

fn create_schema(conn: &Connection) -> Result<(), StoreError> {
    conn.execute(CREATE_FILES, [])?;
    conn.execute(CREATE_CHUNKS, [])?;
    conn.execute(CREATE_EDGE_FILES, [])?;
    conn.execute(CREATE_EDGES, [])?;
    conn.execute(CREATE_EDGES_PATH_INDEX, [])?;
    conn.execute(CREATE_EDGES_SRC_INDEX, [])?;
    conn.execute(CREATE_EDGES_DST_INDEX, [])?;
    conn.execute(CREATE_INDEX_BUILD_STATE, [])?;
    conn.execute(CREATE_ENTITY_SEARCH_WATERMARK, [])?;
    conn.execute(CREATE_SEGMENT_AGGREGATE_MIGRATION, [])?;
    conn.execute(
        "REPLACE INTO edge_files(path, mtime) VALUES (?, ?)",
        params![EDGES_SCHEMA_PATH, EDGES_SCHEMA_VERSION],
    )?;
    Ok(())
}

/// Read the cursor for the one-shot segment aggregate cleanup.
///
/// Absence is normal for a database that has not started the migration yet.
pub fn read_segment_aggregate_migration(
    conn: &Connection,
) -> Result<Option<SegmentAggregateMigration>, StoreError> {
    if !sqlite_table_exists(conn, "segment_aggregate_migration")? {
        return Ok(None);
    }
    conn.query_row(
        "SELECT CAST(cursor AS TEXT), CAST(completed AS INTEGER) FROM segment_aggregate_migration WHERE id=1",
        [],
        |row| {
            Ok(SegmentAggregateMigration {
                cursor: row.get(0)?,
                completed: row.get::<_, i64>(1)? != 0,
            })
        },
    )
    .optional()
    .map_err(StoreError::from)
}

pub fn write_segment_aggregate_migration(
    conn: &Connection,
    cursor: &str,
    completed: bool,
) -> Result<(), StoreError> {
    conn.execute(
        "REPLACE INTO segment_aggregate_migration(id, cursor, completed) VALUES (1, ?, ?)",
        params![cursor, i64::from(completed)],
    )?;
    Ok(())
}

/// Read the entity-search watermark when one has been written by the native indexer.
///
/// Absence is a normal migration/reset state and lets the scanner seed it from
/// a legacy writer once.
pub fn read_entity_search_watermark(
    conn: &Connection,
) -> Result<Option<EntitySearchWatermark>, StoreError> {
    if !sqlite_table_exists(conn, "entity_search_watermark")? {
        return Ok(None);
    }
    let row = conn
        .query_row(
            "SELECT CAST(mtime AS INTEGER), CAST(count AS INTEGER) FROM entity_search_watermark WHERE id=1",
            [],
            |row| Ok((row.get::<_, Option<i64>>(0)?, row.get::<_, Option<i64>>(1)?)),
        )
        .optional()?;
    let Some((Some(mtime), Some(count))) = row else {
        return Ok(None);
    };
    Ok(Some(EntitySearchWatermark { mtime, count }))
}

pub fn write_entity_search_watermark(
    conn: &Connection,
    mtime: i64,
    count: i64,
) -> Result<(), StoreError> {
    conn.execute(
        "REPLACE INTO entity_search_watermark(id, mtime, count) VALUES (1, ?, ?)",
        params![mtime, count],
    )?;
    Ok(())
}

/// Read the index build state when this index was created by a compatible writer.
///
/// Missing, legacy, or malformed state is deliberately reported as unknown rather
/// than failing a read-only query.
pub fn read_index_build_state(conn: &Connection) -> Result<Option<IndexBuildState>, StoreError> {
    if !sqlite_table_exists(conn, "index_build_state")? {
        return Ok(None);
    }
    let row = conn
        .query_row(
            "SELECT CAST(schema_version AS INTEGER), CAST(state AS TEXT), CAST(files_count AS INTEGER), CAST(chunks_count AS INTEGER) FROM index_build_state WHERE id=1",
            [],
            |row| {
                Ok((
                    row.get::<_, Option<i64>>(0)?,
                    row.get::<_, Option<String>>(1)?,
                    row.get::<_, Option<i64>>(2)?,
                    row.get::<_, Option<i64>>(3)?,
                ))
            },
        )
        .optional()?;
    let Some((Some(schema_version), Some(state), Some(files_count), Some(chunks_count))) = row
    else {
        return Ok(None);
    };
    if schema_version != INDEX_BUILD_STATE_SCHEMA_VERSION || files_count < 0 || chunks_count < 0 {
        return Ok(None);
    }
    let state = match state.as_str() {
        "building" => IndexBuildLifecycle::Building,
        "complete" => IndexBuildLifecycle::Complete,
        _ => return Ok(None),
    };
    Ok(Some(IndexBuildState {
        schema_version,
        state,
        files_count,
        chunks_count,
    }))
}

pub(crate) fn mark_index_build_complete(
    tx: &Transaction<'_>,
    files_count: i64,
    chunks_count: i64,
) -> Result<(), StoreError> {
    tx.execute(
        "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, ?, 'complete', ?, ?)",
        params![INDEX_BUILD_STATE_SCHEMA_VERSION, files_count, chunks_count],
    )?;
    Ok(())
}

fn sqlite_table_exists(conn: &Connection, table: &str) -> Result<bool, StoreError> {
    let count: i64 = conn.query_row(
        "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
        [table],
        |row| row.get(0),
    )?;
    Ok(count == 1)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::reserve_temp_path;
    use serde_json::json;

    fn temp_root(name: &str) -> PathBuf {
        reserve_temp_path(&format!("solstone-core-indexer-store-{name}"))
    }

    #[test]
    fn index_readability_does_not_create_a_missing_database() {
        let root = temp_root("index-readable");
        assert!(!is_index_readable(&root));
        assert!(!root.join(INDEX_DIR).exists());

        open_index(&root).expect("create index");
        assert!(is_index_readable(&root));
        fs::remove_dir_all(root).expect("remove test index");
    }

    #[test]
    fn reset_creates_a_building_state_row() {
        let root = temp_root("reset-building-state");
        reset_index(&root).expect("reset index");
        let conn = open_index(&root).expect("open reset index");
        assert_eq!(
            read_index_build_state(&conn).expect("read build state"),
            Some(IndexBuildState {
                schema_version: INDEX_BUILD_STATE_SCHEMA_VERSION,
                state: IndexBuildLifecycle::Building,
                files_count: 0,
                chunks_count: 0,
            })
        );
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read reset entity watermark"),
            None
        );
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup reset state root");
    }

    #[test]
    fn entity_search_watermark_round_trips_and_reset_clears_it() {
        let root = temp_root("entity-search-watermark");
        let conn = open_index(&root).expect("open index");
        write_entity_search_watermark(&conn, 123, 4).expect("write entity watermark");
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read entity watermark"),
            Some(EntitySearchWatermark {
                mtime: 123,
                count: 4,
            })
        );
        drop(conn);

        reset_index(&root).expect("reset index");
        let conn = open_index(&root).expect("open reset index");
        assert_eq!(
            read_entity_search_watermark(&conn).expect("read cleared entity watermark"),
            None
        );
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup entity watermark root");
    }

    #[test]
    fn open_and_stream_prune_do_not_create_a_build_state_row() {
        let root = temp_root("no-build-state-writes");
        let conn = open_index(&root).expect("open index");
        assert_eq!(read_index_build_state(&conn).expect("read state"), None);
        drop(conn);

        prune_chunks_by_stream(&root, "default").expect("prune empty stream");
        let conn = open_index(&root).expect("reopen index");
        assert_eq!(read_index_build_state(&conn).expect("read state"), None);
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup no-state root");
    }

    fn table_schema_json(conn: &Connection, table: &str) -> serde_json::Value {
        let columns = conn
            .prepare(&format!("PRAGMA table_info({table})"))
            .expect("prepare table_info")
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let column_type: String = row.get(2)?;
                let notnull: i64 = row.get(3)?;
                let pk: i64 = row.get(5)?;
                Ok(json!({
                    "name": name,
                    "type": column_type,
                    "notnull": notnull,
                    "pk": pk,
                }))
            })
            .expect("query table_info")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect table_info");

        let mut indexes = conn
            .prepare(&format!("PRAGMA index_list({table})"))
            .expect("prepare index_list")
            .query_map([], |row| {
                let name: String = row.get(1)?;
                let unique: i64 = row.get(2)?;
                let origin: String = row.get(3)?;
                Ok((name, unique, origin))
            })
            .expect("query index_list")
            .collect::<Result<Vec<_>, _>>()
            .expect("collect index_list")
            .into_iter()
            .map(|(name, unique, origin)| {
                let columns = conn
                    .prepare(&format!("PRAGMA index_info({name})"))
                    .expect("prepare index_info")
                    .query_map([], |row| row.get::<_, String>(2))
                    .expect("query index_info")
                    .collect::<Result<Vec<_>, _>>()
                    .expect("collect index_info");
                json!({
                    "name": name,
                    "unique": unique,
                    "origin": origin,
                    "columns": columns,
                })
            })
            .collect::<Vec<_>>();
        indexes.sort_by(|left, right| {
            left["name"]
                .as_str()
                .expect("left index name")
                .cmp(right["name"].as_str().expect("right index name"))
        });

        json!({
            "columns": columns,
            "indexes": indexes,
        })
    }

    fn edge_sentinel_json(conn: &Connection) -> serde_json::Value {
        conn.query_row("SELECT path, mtime FROM edge_files", [], |row| {
            let path: String = row.get(0)?;
            let mtime: i64 = row.get(1)?;
            Ok(json!({
                "path": path,
                "mtime": mtime,
            }))
        })
        .expect("edge schema sentinel")
    }

    fn table_exists(conn: &Connection, table: &str) -> bool {
        let count: i64 = conn
            .query_row(
                "SELECT count(*) FROM sqlite_master WHERE type='table' AND name=?",
                [table],
                |row| row.get(0),
            )
            .expect("table existence query");
        count == 1
    }

    #[test]
    fn creates_schema_and_pragmas() {
        let root = temp_root("schema");
        let conn = open_index(&root).expect("open index");
        let journal_mode: String = conn
            .query_row("PRAGMA journal_mode", [], |row| row.get(0))
            .expect("journal mode");
        assert_eq!(journal_mode, "wal");
        let synchronous: i64 = conn
            .query_row("PRAGMA synchronous", [], |row| row.get(0))
            .expect("synchronous");
        assert_eq!(synchronous, 1);
        let busy_timeout: i64 = conn
            .query_row("PRAGMA busy_timeout", [], |row| row.get(0))
            .expect("busy timeout");
        assert_eq!(busy_timeout, 5000);
        let files_sql: String = conn
            .query_row(
                "SELECT sql FROM sqlite_master WHERE type='table' AND name='files'",
                [],
                |row| row.get(0),
            )
            .expect("files schema");
        assert_eq!(
            files_sql,
            "CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER)"
        );
        let chunk_cols: Vec<String> = conn
            .prepare("PRAGMA table_info(chunks)")
            .expect("prepare table_info")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("query table_info")
            .collect::<Result<_, _>>()
            .expect("collect table_info");
        assert_eq!(
            chunk_cols,
            vec![
                "content",
                "path",
                "day",
                "facet",
                "agent",
                "stream",
                "idx",
                "time_bucket",
            ]
        );
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup schema root");
    }

    #[test]
    fn open_migrates_legacy_chunks_without_losing_rows() {
        for (name, stream_column, expected_stream) in [
            ("legacy-time-bucket", ", stream UNINDEXED", "private"),
            ("legacy-stream", "", ""),
        ] {
            let root = temp_root(name);
            fs::create_dir_all(root.join(INDEX_DIR)).expect("create legacy index dir");
            let conn = Connection::open(db_path(&root)).expect("open legacy index");
            conn.execute_batch(&format!(
                "CREATE VIRTUAL TABLE chunks USING fts5(
                    content,
                    path UNINDEXED,
                    day UNINDEXED,
                    facet UNINDEXED,
                    agent UNINDEXED
                    {stream_column},
                    idx UNINDEXED
                );
                CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER);"
            ))
            .expect("create legacy schema");
            if stream_column.is_empty() {
                conn.execute(
                    "INSERT INTO chunks(content,path,day,facet,agent,idx) VALUES ('remember me','legacy.md','20260809','work','flow',3)",
                    [],
                )
                .expect("insert pre-stream row");
            } else {
                conn.execute(
                    "INSERT INTO chunks(content,path,day,facet,agent,stream,idx) VALUES ('remember me','legacy.md','20260809','work','flow','private',3)",
                    [],
                )
                .expect("insert pre-time-bucket row");
            }
            drop(conn);

            let conn = open_index(&root).expect("migrate legacy index");
            let row = conn
                .query_row(
                    "SELECT content,path,day,facet,agent,stream,idx,time_bucket FROM chunks",
                    [],
                    |row| {
                        Ok((
                            row.get::<_, String>(0)?,
                            row.get::<_, String>(1)?,
                            row.get::<_, String>(2)?,
                            row.get::<_, String>(3)?,
                            row.get::<_, String>(4)?,
                            row.get::<_, String>(5)?,
                            row.get::<_, i64>(6)?,
                            row.get::<_, String>(7)?,
                        ))
                    },
                )
                .expect("read migrated row");
            assert_eq!(
                row,
                (
                    "remember me".to_string(),
                    "legacy.md".to_string(),
                    "20260809".to_string(),
                    "work".to_string(),
                    "flow".to_string(),
                    expected_stream.to_string(),
                    3,
                    String::new(),
                )
            );
            drop(conn);
            fs::remove_dir_all(root).expect("cleanup legacy index");
        }
    }

    #[test]
    fn edge_schema_matches_python_golden() {
        let root = temp_root("edge-golden");
        let conn = open_index(&root).expect("open index");
        let native_tables = json!({
            "edge_files": table_schema_json(&conn, "edge_files"),
            "edges": table_schema_json(&conn, "edges"),
        });
        let native_sentinel = edge_sentinel_json(&conn);
        let native_schema_version = native_sentinel["mtime"].clone();
        let golden: serde_json::Value = serde_json::from_str(include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../fixtures/edge_schema.json"
        )))
        .expect("parse edge schema golden");

        assert_eq!(native_tables, golden["tables"]);
        assert_eq!(native_sentinel, golden["sentinel"]);
        assert_eq!(native_schema_version, golden["schema_version"]);
        assert_eq!(golden["schema_version"], json!(EDGES_SCHEMA_VERSION));

        drop(conn);
        fs::remove_dir_all(root).expect("cleanup edge golden root");
    }

    #[test]
    fn edge_tables_empty_except_sentinel() {
        assert_eq!(EDGES_SCHEMA_PATH, "edges:__schema__");
        assert_eq!(EDGES_SCHEMA_VERSION, 1);

        let root = temp_root("edge-empty");
        let conn = open_index(&root).expect("open index");
        assert!(table_exists(&conn, "edge_files"));
        assert!(table_exists(&conn, "edges"));
        let edges_count: i64 = conn
            .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
            .expect("edges count");
        let edge_files_count: i64 = conn
            .query_row("SELECT count(*) FROM edge_files", [], |row| row.get(0))
            .expect("edge files count");
        let sentinel: (String, i64) = conn
            .query_row("SELECT path, mtime FROM edge_files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("edge schema sentinel");

        assert_eq!(edges_count, 0);
        assert_eq!(edge_files_count, 1);
        assert_eq!(
            sentinel,
            (EDGES_SCHEMA_PATH.to_string(), EDGES_SCHEMA_VERSION)
        );

        drop(conn);
        fs::remove_dir_all(root).expect("cleanup edge empty root");
    }

    #[test]
    fn edge_schema_ensure_is_idempotent() {
        let root = temp_root("edge-idempotent");
        let conn = open_index(&root).expect("first open index");
        drop(conn);

        let conn = open_index(&root).expect("second open index");
        let edges_count: i64 = conn
            .query_row("SELECT count(*) FROM edges", [], |row| row.get(0))
            .expect("edges count");
        let edge_files_count: i64 = conn
            .query_row("SELECT count(*) FROM edge_files", [], |row| row.get(0))
            .expect("edge files count");
        let sentinel: (String, i64) = conn
            .query_row("SELECT path, mtime FROM edge_files", [], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .expect("edge schema sentinel");

        assert_eq!(edges_count, 0);
        assert_eq!(edge_files_count, 1);
        assert_eq!(
            sentinel,
            (EDGES_SCHEMA_PATH.to_string(), EDGES_SCHEMA_VERSION)
        );

        drop(conn);
        fs::remove_dir_all(root).expect("cleanup edge idempotent root");
    }

    #[test]
    fn reset_rebuilds_schema_without_unlinking_database_files() {
        let root = temp_root("reset");
        let conn = open_index(&root).expect("open index");
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('stale', 'stale.md', '', '', 'note', '', 0, '')",
            [],
        )
        .expect("seed chunk");
        conn.execute("REPLACE INTO files(path, mtime) VALUES ('stale.md', 1)", [])
            .expect("seed file");
        conn.execute(
            "INSERT INTO edges(src, dst, kind, directed, source, path, weight) VALUES ('a', 'b', 'related', 0, 'test', 'edge.jsonl', 1)",
            [],
        )
        .expect("seed edge");
        assert!(db_path(&root).is_file());
        assert!(root.join(INDEX_DIR).join("journal.sqlite-wal").is_file());
        assert!(root.join(INDEX_DIR).join("journal.sqlite-shm").is_file());
        reset_index(&root).expect("reset index");
        assert!(db_path(&root).is_file());
        assert!(root.join(INDEX_DIR).join("journal.sqlite-wal").is_file());
        assert!(root.join(INDEX_DIR).join("journal.sqlite-shm").is_file());

        let reset_conn = Connection::open(db_path(&root)).expect("open reset db");
        assert!(table_exists(&reset_conn, "files"));
        assert!(table_exists(&reset_conn, "chunks"));
        assert!(table_exists(&reset_conn, "edge_files"));
        assert!(table_exists(&reset_conn, "edges"));
        assert_eq!(count_rows(&reset_conn, "files"), 0);
        assert_eq!(count_rows(&reset_conn, "chunks"), 0);
        assert_eq!(count_rows(&reset_conn, "edges"), 0);
        assert_eq!(
            edge_sentinel_json(&reset_conn),
            json!({"path": EDGES_SCHEMA_PATH, "mtime": EDGES_SCHEMA_VERSION})
        );
        assert_sqlite_integrity(&reset_conn);
        drop(reset_conn);
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup reset root");
    }

    #[test]
    fn reset_recovers_from_orphaned_fts_shadow_tables() {
        let root = temp_root("reset-orphan-shadow");
        fs::create_dir_all(root.join(INDEX_DIR)).expect("create index dir");
        let conn = Connection::open(db_path(&root)).expect("open incomplete db");
        conn.execute_batch(
            "\
CREATE TABLE files(path TEXT PRIMARY KEY, mtime INTEGER);
CREATE TABLE chunks_config(k PRIMARY KEY, v) WITHOUT ROWID;
CREATE TABLE chunks_docsize(id INTEGER PRIMARY KEY, sz BLOB);
CREATE TABLE chunks_content(id INTEGER PRIMARY KEY, c0, c1, c2, c3, c4, c5, c6, c7);
CREATE TABLE chunks_idx(segid, term, pgno, PRIMARY KEY(segid, term)) WITHOUT ROWID;
CREATE TABLE chunks_data(id INTEGER PRIMARY KEY, block BLOB);
CREATE TABLE edge_files(path TEXT PRIMARY KEY, mtime INTEGER);
",
        )
        .expect("seed incomplete schema");
        drop(conn);

        reset_index(&root).expect("reset incomplete schema");
        let conn = Connection::open(db_path(&root)).expect("open reset db");
        assert!(table_exists(&conn, "chunks"));
        assert!(table_exists(&conn, "files"));
        assert!(table_exists(&conn, "edges"));
        assert_eq!(
            edge_sentinel_json(&conn),
            json!({"path": EDGES_SCHEMA_PATH, "mtime": EDGES_SCHEMA_VERSION})
        );
        assert_sqlite_integrity(&conn);
        fs::remove_dir_all(root).expect("cleanup reset root");
    }

    #[test]
    fn prunes_only_the_requested_stream_and_its_file_rows() {
        let root = temp_root("stream-prune");
        let conn = open_index(&root).expect("open index");
        for (path, stream) in [("location.md", "location"), ("pixel.md", "pixel")] {
            conn.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) VALUES ('content', ?, '', '', 'test', ?, 0, '')",
                [path, stream],
            )
            .expect("seed chunk");
            conn.execute("INSERT INTO files(path, mtime) VALUES (?, 1)", [path])
                .expect("seed file");
        }
        drop(conn);

        assert_eq!(
            prune_chunks_by_stream(&root, "location").expect("prune stream"),
            StreamPruneCounts {
                chunks: 1,
                files: 1,
            }
        );
        let conn = open_index(&root).expect("reopen index");
        assert_eq!(count_rows(&conn, "chunks"), 1);
        assert_eq!(count_rows(&conn, "files"), 1);
        let stream: String = conn
            .query_row("SELECT stream FROM chunks", [], |row| row.get(0))
            .expect("remaining stream");
        assert_eq!(stream, "pixel");
        drop(conn);
        fs::remove_dir_all(root).expect("cleanup stream prune root");
    }

    fn count_rows(conn: &Connection, table: &str) -> i64 {
        conn.query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
            row.get(0)
        })
        .expect("count rows")
    }

    fn assert_sqlite_integrity(conn: &Connection) {
        let integrity: String = conn
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .expect("integrity check");
        assert_eq!(integrity, "ok");
        conn.execute("INSERT INTO chunks(chunks) VALUES('integrity-check')", [])
            .expect("fts integrity check");
    }

    #[test]
    fn prune_by_paths_clears_a_segment_and_everything_inside_it() {
        let journal = temp_root("prune-by-paths");
        {
            let conn = open_index(&journal).unwrap();
            conn.execute(
                "INSERT INTO files(path, mtime) VALUES (?1, 1)",
                ["chronicle/20260805/field.audio/070000_17/audio.jsonl"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO files(path, mtime) VALUES (?1, 1)",
                ["chronicle/20260805/field.audio/070100_17/audio.jsonl"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES ('a', ?1, '20260805', '', '', 'field.audio', 0, '')",
                ["chronicle/20260805/field.audio/070000_17/audio.jsonl"],
            )
            .unwrap();
            conn.execute(
                "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
                 VALUES ('b', ?1, '20260805', '', '', 'field.audio', 0, '')",
                ["chronicle/20260805/field.audio/070100_17/audio.jsonl"],
            )
            .unwrap();
        }

        let counts = prune_by_paths(&journal, &["chronicle/20260805/field.audio/070000_17"])
            .unwrap()
            .expect("the journal has an index");
        assert_eq!(counts.chunks, 1);
        assert_eq!(counts.files, 1);

        let conn = open_index(&journal).unwrap();
        let remaining: i64 = conn
            .query_row("SELECT COUNT(*) FROM files", [], |row| row.get(0))
            .unwrap();
        assert_eq!(remaining, 1, "the sibling segment survives");
        let surviving: String = conn
            .query_row("SELECT path FROM files", [], |row| row.get(0))
            .unwrap();
        assert!(surviving.contains("070100_17"));
        fs::remove_dir_all(&journal).unwrap();
    }

    /// A path the index never held is ordinary, not an error.
    #[test]
    fn prune_by_paths_tolerates_paths_it_never_held() {
        let journal = temp_root("prune-unknown");
        open_index(&journal).unwrap();
        let counts = prune_by_paths(&journal, &["chronicle/20260805/field.audio/nosuch"])
            .unwrap()
            .expect("the journal has an index");
        assert_eq!(counts, StreamPruneCounts::default());
        fs::remove_dir_all(&journal).unwrap();
    }

    /// ⛔ A prune must never be the thing that creates an index.
    #[test]
    fn prune_by_paths_does_not_create_an_index() {
        let journal = temp_root("prune-no-index");
        // A journal that exists but has never been indexed -- the realistic case.
        fs::create_dir_all(&journal).unwrap();
        assert!(
            prune_by_paths(&journal, &["chronicle/20260805/field.audio/070000_17"])
                .unwrap()
                .is_none(),
            "a journal with no index reports no counts"
        );
        assert!(
            !db_path(&journal).exists(),
            "the prune must not have materialised a database"
        );
        fs::remove_dir_all(&journal).unwrap();
    }

    fn seed_chunk_and_file(conn: &Connection, path: &str, content: &str) {
        conn.execute("INSERT INTO files(path, mtime) VALUES (?1, 1)", [path])
            .unwrap();
        conn.execute(
            "INSERT INTO chunks(content, path, day, facet, agent, stream, idx, time_bucket) \
             VALUES (?1, ?2, '20260508', '', '', '', 0, '')",
            [content, path],
        )
        .unwrap();
    }

    fn count_path(conn: &Connection, table: &str, path: &str) -> i64 {
        conn.query_row(
            &format!("SELECT COUNT(*) FROM {table} WHERE path=?1"),
            [path],
            |row| row.get(0),
        )
        .unwrap()
    }

    #[test]
    fn prune_authored_chat_paths_deletes_both_path_shapes_and_leaves_controls() {
        let journal = temp_root("prune-authored-chat");
        let conn = open_index(&journal).unwrap();
        seed_chunk_and_file(
            &conn,
            "20260508/chat/120000_300/chat.jsonl",
            "NeedADiffNullStream",
        );
        seed_chunk_and_file(
            &conn,
            "chronicle/20260509/chat/120000_300/chat.jsonl",
            "NeedADiffChatStream",
        );
        seed_chunk_and_file(&conn, "20260508/talents/chat.md", "TalentChatMdControl");
        seed_chunk_and_file(
            &conn,
            "20260508/import.chatgpt/thread/conversation_transcript.jsonl",
            "ImportChatgptControl",
        );
        seed_chunk_and_file(
            &conn,
            "facets/chat/logs/chat.jsonl",
            "FacetsChatActionLogControl",
        );
        drop(conn);

        let counts = prune_authored_chat_paths(&journal)
            .unwrap()
            .expect("the journal has an index");
        assert_eq!(counts.chunks, 2);
        assert_eq!(counts.files, 2);

        let conn = open_index(&journal).unwrap();
        assert_eq!(
            count_path(&conn, "chunks", "20260508/chat/120000_300/chat.jsonl"),
            0
        );
        assert_eq!(
            count_path(&conn, "files", "20260508/chat/120000_300/chat.jsonl"),
            0
        );
        assert_eq!(
            count_path(
                &conn,
                "chunks",
                "chronicle/20260509/chat/120000_300/chat.jsonl"
            ),
            0
        );
        assert_eq!(
            count_path(
                &conn,
                "files",
                "chronicle/20260509/chat/120000_300/chat.jsonl"
            ),
            0
        );
        assert_eq!(count_path(&conn, "chunks", "20260508/talents/chat.md"), 1);
        assert_eq!(count_path(&conn, "files", "20260508/talents/chat.md"), 1);
        assert_eq!(
            count_path(
                &conn,
                "chunks",
                "20260508/import.chatgpt/thread/conversation_transcript.jsonl"
            ),
            1
        );
        assert_eq!(
            count_path(
                &conn,
                "files",
                "20260508/import.chatgpt/thread/conversation_transcript.jsonl"
            ),
            1
        );
        assert_eq!(
            count_path(&conn, "chunks", "facets/chat/logs/chat.jsonl"),
            1
        );
        assert_eq!(count_path(&conn, "files", "facets/chat/logs/chat.jsonl"), 1);
        fs::remove_dir_all(&journal).unwrap();
    }

    #[test]
    fn prune_authored_chat_paths_does_not_create_an_index() {
        let journal = temp_root("prune-authored-chat-no-index");
        fs::create_dir_all(&journal).unwrap();
        assert!(
            prune_authored_chat_paths(&journal).unwrap().is_none(),
            "a journal with no index reports no counts"
        );
        assert!(
            !db_path(&journal).exists(),
            "the prune must not have materialised a database"
        );
        fs::remove_dir_all(&journal).unwrap();
    }
}
