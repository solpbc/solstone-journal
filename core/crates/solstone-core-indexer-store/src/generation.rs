// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The index generation stamp.
//!
//! `journal.sqlite` carries three marks: `PRAGMA application_id` (this file is a
//! journal search index), `PRAGMA user_version` (its generation) and the
//! `index_meta` rows `generation`, `last_compatible_generation` and
//! `writer_version`. A binary never writes an index whose generation is newer
//! than the one it knows. An unstamped index is generation 1.
//!
//! Stamping is metadata only: it never rebuilds, rewrites or reads chunks. It
//! happens only on paths that already hold the writer lease, and only when a
//! value differs, so a steady-state writer adds no write. Readers never stamp.
//!
//! An older binary never reads or writes these marks: it ignores `index_meta`
//! and leaves the header values in place, so its writes are harmless to the
//! stamp. `writer_version` therefore names the last *stamping* writer, not
//! necessarily the last writer.

use std::io::Read;
use std::path::Path;

use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior, params};

use crate::StoreError;
use crate::db::sqlite_table_exists;
use crate::scan::ScanFailure;

/// `PRAGMA application_id` of a journal search index ("SSJI").
pub const INDEX_APPLICATION_ID: i32 = 0x5353_4A49;
/// The index generation this binary reads and writes.
pub const INDEX_GENERATION: i64 = 1;
/// The oldest generation a binary may know and still read what this one writes.
pub const INDEX_LAST_COMPATIBLE_GENERATION: i64 = 1;
/// The version recorded by every admitted write.
pub const WRITER_VERSION: &str = env!("CARGO_PKG_VERSION");

const CREATE_INDEX_META: &str =
    "CREATE TABLE IF NOT EXISTS index_meta(key TEXT PRIMARY KEY, value TEXT NOT NULL)";
const META_GENERATION: &str = "generation";
const META_LAST_COMPATIBLE: &str = "last_compatible_generation";
const META_WRITER_VERSION: &str = "writer_version";
const META_LAST_SCAN: &str = "last_scan";

/// The stamp as found in a file, before any defaulting.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexStamp {
    pub application_id: i32,
    pub user_version: i64,
    pub generation: Option<i64>,
    pub last_compatible_generation: Option<i64>,
    pub writer_version: Option<String>,
}

impl IndexStamp {
    /// Whether any mark has been written.
    #[must_use]
    pub fn is_stamped(&self) -> bool {
        self.application_id != 0
            || self.user_version != 0
            || self.generation.is_some()
            || self.last_compatible_generation.is_some()
            || self.writer_version.is_some()
    }

    /// The file's generation. Unstamped is generation 1; when the header and
    /// the metadata row disagree, the newer one wins.
    #[must_use]
    pub fn effective_generation(&self) -> i64 {
        self.user_version.max(self.generation.unwrap_or(0)).max(1)
    }

    /// The oldest generation that may still read this file.
    #[must_use]
    pub fn effective_last_compatible_generation(&self) -> i64 {
        self.last_compatible_generation
            .unwrap_or_else(|| self.effective_generation())
    }

    /// Whether this binary may write the file.
    #[must_use]
    pub fn writable(&self) -> bool {
        self.effective_generation() <= INDEX_GENERATION
    }

    /// Whether this binary may read the file.
    #[must_use]
    pub fn readable(&self) -> bool {
        self.effective_last_compatible_generation() <= INDEX_GENERATION
    }

    fn current(&self) -> bool {
        self.application_id == INDEX_APPLICATION_ID
            && self.user_version == INDEX_GENERATION
            && self.generation == Some(INDEX_GENERATION)
            && self.last_compatible_generation == Some(INDEX_LAST_COMPATIBLE_GENERATION)
            && self.writer_version.as_deref() == Some(WRITER_VERSION)
    }
}

/// Read the stamp without writing.
pub fn read_stamp(conn: &Connection) -> Result<IndexStamp, StoreError> {
    let application_id = conn.query_row("PRAGMA application_id", [], |row| row.get(0))?;
    let user_version = conn.query_row("PRAGMA user_version", [], |row| row.get(0))?;
    let mut stamp = IndexStamp {
        application_id,
        user_version,
        ..IndexStamp::default()
    };
    let meta = (|| -> Result<_, StoreError> {
        if !sqlite_table_exists(conn, "index_meta")? {
            return Ok((None, None, None));
        }
        Ok((
            read_meta(conn, META_GENERATION)?.and_then(|v| v.parse().ok()),
            read_meta(conn, META_LAST_COMPATIBLE)?.and_then(|v| v.parse().ok()),
            read_meta(conn, META_WRITER_VERSION)?,
        ))
    })();
    match meta {
        Ok((generation, last_compatible_generation, writer_version)) => {
            stamp.generation = generation;
            stamp.last_compatible_generation = last_compatible_generation;
            stamp.writer_version = writer_version;
            Ok(stamp)
        }
        // The header alone already says the file is from a newer generation,
        // whatever shape that generation gave its metadata.
        Err(_) if user_version > INDEX_GENERATION => Ok(stamp),
        Err(error) => Err(error),
    }
}

fn read_meta(conn: &Connection, key: &str) -> Result<Option<String>, StoreError> {
    Ok(conn
        .query_row("SELECT value FROM index_meta WHERE key=?", [key], |row| {
            row.get(0)
        })
        .optional()?)
}

/// Refuse a newer generation, else bring the stamp current. Writes only when a
/// value differs. The caller holds the writer lease and owns the transaction.
pub(crate) fn stamp_if_needed(conn: &Connection) -> Result<(), StoreError> {
    let stamp = read_stamp(conn)?;
    if !stamp.writable() {
        return Err(StoreError::IndexGenerationNewer {
            found: stamp.effective_generation(),
            known: INDEX_GENERATION,
        });
    }
    if stamp.current() {
        return Ok(());
    }
    if stamp.application_id != INDEX_APPLICATION_ID {
        conn.execute_batch(&format!("PRAGMA application_id={INDEX_APPLICATION_ID}"))?;
    }
    if stamp.user_version != INDEX_GENERATION {
        conn.execute_batch(&format!("PRAGMA user_version={INDEX_GENERATION}"))?;
    }
    conn.execute(CREATE_INDEX_META, [])?;
    for (key, value) in [
        (META_GENERATION, INDEX_GENERATION.to_string()),
        (
            META_LAST_COMPATIBLE,
            INDEX_LAST_COMPATIBLE_GENERATION.to_string(),
        ),
        (META_WRITER_VERSION, WRITER_VERSION.to_owned()),
    ] {
        conn.execute(
            "INSERT INTO index_meta(key, value) VALUES (?, ?) \
             ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value != excluded.value",
            params![key, value],
        )?;
    }
    Ok(())
}

/// The writer-lease gate for an index file that already exists.
///
/// A newer generation refuses the write, and so does a stamp that cannot be
/// read or brought current: a writer that cannot tell the generation does not
/// write. The two repairs that replace the file outright (reset, legacy
/// removal) proceed past a damaged stamp, so a broken index can still be
/// rebuilt; reset then writes a fresh stamp.
pub(crate) fn admit_writer(db_path: &Path, operation: &str) -> Result<(), StoreError> {
    // A file that is not a SQLite database has nothing to stamp, and opening
    // one, even read-only, can create sidecars beside it.
    if !has_sqlite_header(db_path) {
        return Ok(());
    }
    match stamp_existing(db_path) {
        Ok(()) => Ok(()),
        Err(error @ StoreError::IndexGenerationNewer { .. }) => Err(error),
        Err(error)
            if matches!(
                operation,
                "reset" | "remove-legacy-index" | "migrate-index-stream"
            ) =>
        {
            log::warn!(
                target: "solstone::indexer",
                "index generation stamp unreadable, proceeding with {operation} path={} cause={error}",
                db_path.display()
            );
            Ok(())
        }
        Err(error) => Err(error),
    }
}

/// Whether the file begins with the SQLite header. Reads 16 bytes, opens nothing.
pub fn has_sqlite_header(db_path: &Path) -> bool {
    let mut header = [0_u8; 16];
    std::fs::File::open(db_path)
        .and_then(|mut file| file.read_exact(&mut header))
        .is_ok_and(|()| &header == b"SQLite format 3\0")
}

fn stamp_existing(db_path: &Path) -> Result<(), StoreError> {
    // Read first through a read-only connection, so an index whose stamp is
    // already current, or newer, is never opened for writing here.
    let stamp = {
        let conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")?;
        read_stamp(&conn)?
    };
    if !stamp.writable() {
        return Err(StoreError::IndexGenerationNewer {
            found: stamp.effective_generation(),
            known: INDEX_GENERATION,
        });
    }
    if stamp.current() {
        return Ok(());
    }
    let mut conn = Connection::open_with_flags(db_path, OpenFlags::SQLITE_OPEN_READ_WRITE)?;
    conn.execute_batch("PRAGMA busy_timeout=5000;")?;
    let tx = conn.transaction_with_behavior(TransactionBehavior::Immediate)?;
    stamp_if_needed(&tx)?;
    tx.commit()?;
    Ok(())
}

/// What the last completed scan reported. Written by the scan, under the lease.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct ScanReceipt {
    pub finished_at_ms: i64,
    pub full: bool,
    pub indexed: usize,
    pub removed: usize,
    pub skipped: usize,
    pub failed: usize,
    pub warnings: usize,
    /// The failures the scan attributed to a file, at most
    /// [`RECEIPT_FAILED_PATHS`]. `failed` beyond these is unattributed.
    pub failures: Vec<ScanFailure>,
    /// Why the scan stopped, when it did not finish.
    pub error: Option<String>,
    pub writer_version: String,
}

/// The most failed paths one receipt names; the count is always exact.
pub const RECEIPT_FAILED_PATHS: usize = 1000;

pub(crate) fn record_scan(conn: &Connection, receipt: &ScanReceipt) -> Result<(), StoreError> {
    conn.execute(CREATE_INDEX_META, [])?;
    let value = serde_json::json!({
        "finished_at_ms": receipt.finished_at_ms,
        "full": receipt.full,
        "indexed": receipt.indexed,
        "removed": receipt.removed,
        "skipped": receipt.skipped,
        "failed": receipt.failed,
        "warnings": receipt.warnings,
        "failures": receipt
            .failures
            .iter()
            .filter_map(|failure| {
                failure
                    .path
                    .as_ref()
                    .map(|path| serde_json::json!({"path": path, "mtime": failure.mtime}))
            })
            .take(RECEIPT_FAILED_PATHS)
            .collect::<Vec<_>>(),
        "error": receipt.error,
        "writer_version": receipt.writer_version,
    });
    conn.execute(
        "REPLACE INTO index_meta(key, value) VALUES (?, ?)",
        params![META_LAST_SCAN, value.to_string()],
    )?;
    Ok(())
}

/// Read the last scan receipt, if a stamping writer recorded one.
pub fn read_last_scan(conn: &Connection) -> Result<Option<ScanReceipt>, StoreError> {
    if !sqlite_table_exists(conn, "index_meta")? {
        return Ok(None);
    }
    let Some(raw) = read_meta(conn, META_LAST_SCAN)? else {
        return Ok(None);
    };
    let Ok(value) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return Ok(None);
    };
    let count = |key: &str| value[key].as_u64().unwrap_or(0) as usize;
    let Some(finished_at_ms) = value["finished_at_ms"].as_i64() else {
        return Ok(None);
    };
    Ok(Some(ScanReceipt {
        finished_at_ms,
        full: value["full"].as_bool().unwrap_or(false),
        indexed: count("indexed"),
        removed: count("removed"),
        skipped: count("skipped"),
        failed: count("failed"),
        warnings: count("warnings"),
        failures: value["failures"]
            .as_array()
            .map(|failures| {
                failures
                    .iter()
                    .filter_map(|failure| {
                        Some(ScanFailure {
                            path: Some(failure["path"].as_str()?.to_owned()),
                            mtime: failure["mtime"].as_i64(),
                        })
                    })
                    .collect()
            })
            .unwrap_or_default(),
        error: value["error"].as_str().map(str::to_owned),
        writer_version: value["writer_version"].as_str().unwrap_or("").to_owned(),
    }))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use super::*;
    use crate::db::{db_path, prune_by_paths, reset_index};
    use crate::scan::{rescan_file, scan_journal};
    use crate::status::inspect_index;
    use crate::test_support::reserve_temp_path;

    struct Journal(PathBuf);

    impl Journal {
        fn with_one_note(name: &str) -> Self {
            let root = reserve_temp_path(&format!("solstone-core-indexer-store-generation-{name}"));
            let path = root.join("chronicle/20260717/talents/flow.md");
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(path, "# Flow\n\none").expect("write note");
            Self(root)
        }
    }

    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn chunks(journal: &Path) -> Vec<(i64, String, String)> {
        let conn = Connection::open(db_path(journal)).expect("open");
        let mut statement = conn
            .prepare("SELECT rowid, path, content FROM chunks ORDER BY rowid")
            .expect("prepare");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
            .expect("query")
            .collect::<Result<_, _>>()
            .expect("rows")
    }

    #[test]
    fn an_unstamped_index_is_generation_one_and_stamping_touches_no_chunk() {
        let journal = Journal::with_one_note("unstamped");
        scan_journal(&journal.0, true).expect("scan");
        // What an index written before the stamp existed looks like.
        Connection::open(db_path(&journal.0))
            .expect("open")
            .execute_batch("DROP TABLE index_meta; PRAGMA user_version=0; PRAGMA application_id=0;")
            .expect("strip stamp");
        let unstamped =
            read_stamp(&Connection::open(db_path(&journal.0)).expect("open")).expect("read stamp");
        assert!(!unstamped.is_stamped());
        assert_eq!(unstamped.effective_generation(), 1);
        let before = chunks(&journal.0);

        // The smallest admitted write: it takes the lease and changes nothing else.
        prune_by_paths(&journal.0, &[]).expect("admitted write");

        let stamp =
            read_stamp(&Connection::open(db_path(&journal.0)).expect("open")).expect("read stamp");
        assert_eq!(stamp.application_id, INDEX_APPLICATION_ID);
        assert_eq!(stamp.user_version, 1);
        assert_eq!(stamp.generation, Some(1));
        assert_eq!(stamp.last_compatible_generation, Some(1));
        assert_eq!(stamp.writer_version.as_deref(), Some(WRITER_VERSION));
        assert_eq!(chunks(&journal.0), before);
    }

    #[test]
    fn a_stamp_that_cannot_be_read_refuses_writes_except_the_rebuilding_reset() {
        let journal = Journal::with_one_note("unreadable-stamp");
        scan_journal(&journal.0, true).expect("scan");
        // A metadata table this binary cannot read the stamp from.
        Connection::open(db_path(&journal.0))
            .expect("open")
            .execute_batch("DROP TABLE index_meta; CREATE TABLE index_meta(key TEXT PRIMARY KEY);")
            .expect("damage the stamp table");
        let before = fs::read(db_path(&journal.0)).expect("bytes");

        assert!(matches!(
            scan_journal(&journal.0, false),
            Err(StoreError::Sql(_))
        ));
        assert_eq!(fs::read(db_path(&journal.0)).expect("bytes"), before);
        // Search and the file table still read, so status keeps its counts.
        let status = inspect_index(&journal.0).expect("status");
        assert!(status.stamp_error.is_some());
        assert_eq!((status.current, status.missing.len()), (1, 0));

        reset_index(&journal.0).expect("a reset rebuilds past a damaged stamp");
        let stamp = read_stamp(&Connection::open(db_path(&journal.0)).expect("open"))
            .expect("read the fresh stamp");
        assert_eq!(stamp.generation, Some(1));
        assert_eq!(stamp.writer_version.as_deref(), Some(WRITER_VERSION));
    }

    #[test]
    fn a_newer_generation_refuses_every_write_and_stays_readable() {
        let journal = Journal::with_one_note("newer");
        scan_journal(&journal.0, true).expect("scan");
        Connection::open(db_path(&journal.0))
            .expect("open")
            .execute_batch("PRAGMA user_version=2;")
            .expect("simulate a newer writer");
        let before = fs::read(db_path(&journal.0)).expect("bytes");

        let refused = |result: Result<(), StoreError>| {
            assert!(
                matches!(
                    result,
                    Err(StoreError::IndexGenerationNewer { found: 2, known: 1 })
                ),
                "{result:?}"
            );
        };
        refused(scan_journal(&journal.0, false).map(|_| ()));
        refused(
            rescan_file(
                &journal.0,
                &journal.0.join("chronicle/20260717/talents/flow.md"),
            )
            .map(|_| ()),
        );
        refused(reset_index(&journal.0));
        assert_eq!(fs::read(db_path(&journal.0)).expect("bytes"), before);

        let status = inspect_index(&journal.0).expect("status still reads");
        let stamp = status.stamp.expect("stamp");
        assert!(!stamp.writable());
        assert!(stamp.readable());
        assert_eq!(status.current, 1);

        // The header decides even when the newer metadata cannot be parsed:
        // neither the rebuilding reset nor legacy removal touches the file.
        Connection::open(db_path(&journal.0))
            .expect("open")
            .execute_batch("DROP TABLE index_meta; CREATE TABLE index_meta(key TEXT PRIMARY KEY);")
            .expect("give the newer generation a metadata shape this binary cannot read");
        let newer_shape = fs::read(db_path(&journal.0)).expect("bytes");
        refused(reset_index(&journal.0));
        refused(
            crate::migrations::index_stream::remove_legacy_index_artifacts(&journal.0).map(|_| ()),
        );
        assert_eq!(fs::read(db_path(&journal.0)).expect("bytes"), newer_shape);
    }
}
