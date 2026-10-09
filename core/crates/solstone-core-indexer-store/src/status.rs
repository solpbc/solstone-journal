// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only index status: how far the index is from the files on disk.
//!
//! Opens the index read-only, never creates it, never takes the writer lease and
//! never writes. Walks the journal with the scan's own discovery and stat rule
//! and compares against the `files` table. Reads no journal content: the only
//! bytes read besides the index are per-directory `shape.json` sidecars, and
//! only for discovered paths the index has no row for.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use rusqlite::{Connection, OpenFlags};
use serde_json::json;
use solstone_core_format::content::{ContentResolution, resolve_content_shape};
use solstone_core_indexer::discovery::discover_indexable_files;

use crate::StoreError;
use crate::chunk_sources::chunk_path_lookup_ready;
use crate::db::{
    IndexBuildLifecycle, classification_facets_missing, db_path,
    read_chunk_classification_backfill, read_index_build_state,
};
use crate::generation::{
    INDEX_GENERATION, IndexStamp, ScanReceipt, WRITER_VERSION, has_sqlite_header, read_last_scan,
    read_stamp,
};
use crate::scan::{file_mtime_secs, is_memory_note, load_file_mtimes, memory_file_mtime_secs};

const SAMPLE_LIMIT: usize = 5;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexDatabase {
    Absent,
    Unreadable(String),
    Present,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ClassificationProgress {
    /// No classification tables yet.
    Absent,
    Complete,
    Incomplete,
    Stalled {
        path: Option<String>,
    },
}

/// The index measured against the disk.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct IndexStatus {
    pub database: IndexDatabase,
    pub stamp: Option<IndexStamp>,
    /// `None` when the index has no recognizable build-state row.
    pub build: Option<IndexBuildLifecycle>,
    pub last_scan: Option<ScanReceipt>,
    pub classification: ClassificationProgress,
    /// Classification memberships are missing: every write refuses until a reset.
    pub memberships_missing: bool,
    pub path_lookup_ready: bool,
    /// Indexable files found on disk.
    pub discovered: usize,
    /// Rows in the index's file table.
    pub indexed: usize,
    /// On disk and indexed at the same modification time.
    pub current: usize,
    /// On disk, indexed, but modified since.
    pub stale: BTreeSet<String>,
    /// On disk, eligible, not indexed.
    pub missing: BTreeSet<String>,
    /// Indexed, no longer on disk.
    pub orphaned: BTreeSet<String>,
    /// On disk, but its modification time could not be read.
    pub unreadable: BTreeSet<String>,
    /// On disk, not indexed, and not meant to be (raw media, an unready note).
    pub ineligible: usize,
}

impl IndexStatus {
    /// Changes on disk the index has not taken in yet.
    #[must_use]
    pub fn pending(&self) -> usize {
        self.stale.len() + self.missing.len() + self.orphaned.len()
    }

    /// Whether this binary may write the index.
    #[must_use]
    pub fn writable(&self) -> bool {
        self.stamp.as_ref().is_none_or(IndexStamp::writable)
    }

    #[must_use]
    pub fn to_json_value(&self) -> serde_json::Value {
        let database = match &self.database {
            IndexDatabase::Absent => json!({"state": "absent"}),
            IndexDatabase::Unreadable(reason) => json!({"state": "unreadable", "reason": reason}),
            IndexDatabase::Present => json!({"state": "present"}),
        };
        let generation = self.stamp.as_ref().map(|stamp| {
            json!({
                "generation": stamp.effective_generation(),
                "last_compatible_generation": stamp.effective_last_compatible_generation(),
                "stamped": stamp.is_stamped(),
                "writer_version": stamp.writer_version,
                "known_generation": INDEX_GENERATION,
                "this_version": WRITER_VERSION,
                "readable": stamp.readable(),
                "writable": stamp.writable(),
            })
        });
        let classification = match &self.classification {
            ClassificationProgress::Absent => json!({"state": "absent"}),
            ClassificationProgress::Complete => json!({"state": "complete"}),
            ClassificationProgress::Incomplete => json!({"state": "incomplete"}),
            ClassificationProgress::Stalled { path } => json!({"state": "stalled", "path": path}),
        };
        let last_scan = self.last_scan.as_ref().map(|scan| {
            json!({
                "finished_at_ms": scan.finished_at_ms,
                "full": scan.full,
                "indexed": scan.indexed,
                "removed": scan.removed,
                "skipped": scan.skipped,
                "failed": scan.failed,
                "warnings": scan.warnings,
                "writer_version": scan.writer_version,
            })
        });
        json!({
            "database": database,
            "generation": generation,
            "build": self.build.map(|build| match build {
                IndexBuildLifecycle::Building => "building",
                IndexBuildLifecycle::Complete => "complete",
            }),
            "last_scan": last_scan,
            "classification": classification,
            "memberships_missing": self.memberships_missing,
            "path_lookup_ready": self.path_lookup_ready,
            "files": {
                "discovered": self.discovered,
                "indexed": self.indexed,
                "current": self.current,
                "stale": self.stale.len(),
                "missing": self.missing.len(),
                "orphaned": self.orphaned.len(),
                "unreadable": self.unreadable.len(),
                "ineligible": self.ineligible,
            },
            "samples": {
                "stale": sample(&self.stale),
                "missing": sample(&self.missing),
                "orphaned": sample(&self.orphaned),
                "unreadable": sample(&self.unreadable),
            },
        })
    }

    #[must_use]
    pub fn format_human(&self) -> String {
        let mut out = String::new();
        match &self.database {
            IndexDatabase::Absent => out.push_str("index: absent\n"),
            IndexDatabase::Unreadable(reason) => {
                out.push_str(&format!("index: unreadable ({reason})\n"));
            }
            IndexDatabase::Present => out.push_str("index: present\n"),
        }
        if let Some(stamp) = &self.stamp {
            out.push_str(&format!(
                "generation: {} (this version knows {}; writable: {})\n",
                stamp.effective_generation(),
                INDEX_GENERATION,
                if stamp.writable() { "yes" } else { "no" }
            ));
        }
        out.push_str(&format!(
            "files: {} on disk, {} indexed, {} current, {} stale, {} missing, {} orphaned, {} unreadable, {} not indexable\n",
            self.discovered,
            self.indexed,
            self.current,
            self.stale.len(),
            self.missing.len(),
            self.orphaned.len(),
            self.unreadable.len(),
            self.ineligible
        ));
        let build = match self.build {
            Some(IndexBuildLifecycle::Building) => "building",
            Some(IndexBuildLifecycle::Complete) => "complete",
            None => "unknown",
        };
        out.push_str(&format!("build: {build}\n"));
        let classification = match &self.classification {
            ClassificationProgress::Absent => "absent".to_owned(),
            ClassificationProgress::Complete => "complete".to_owned(),
            ClassificationProgress::Incomplete => "incomplete".to_owned(),
            ClassificationProgress::Stalled { path } => {
                format!("stalled at {}", path.as_deref().unwrap_or("unknown path"))
            }
        };
        out.push_str(&format!("classification: {classification}\n"));
        match &self.last_scan {
            Some(scan) => out.push_str(&format!(
                "last scan: {} at {} ms, indexed {}, removed {}, failed {}\n",
                if scan.full { "full" } else { "light" },
                scan.finished_at_ms,
                scan.indexed,
                scan.removed,
                scan.failed
            )),
            None => out.push_str("last scan: not recorded\n"),
        }
        out
    }
}

fn sample(paths: &BTreeSet<String>) -> Vec<&str> {
    paths
        .iter()
        .take(SAMPLE_LIMIT)
        .map(String::as_str)
        .collect()
}

/// Measure the index against the disk without changing either.
pub fn inspect_index(journal: &Path) -> Result<IndexStatus, StoreError> {
    let files = discover_indexable_files(journal)?;
    let mut status = IndexStatus {
        database: IndexDatabase::Absent,
        stamp: None,
        build: None,
        last_scan: None,
        classification: ClassificationProgress::Absent,
        memberships_missing: false,
        path_lookup_ready: false,
        discovered: files.len(),
        indexed: 0,
        current: 0,
        stale: BTreeSet::new(),
        missing: BTreeSet::new(),
        orphaned: BTreeSet::new(),
        unreadable: BTreeSet::new(),
        ineligible: 0,
    };

    let path = db_path(journal);
    let stored = if path.is_file() && !has_sqlite_header(&path) {
        // Opening a file that is not a database, even read-only, can leave
        // sidecars beside it; a status never does.
        status.database = IndexDatabase::Unreadable("not a SQLite database".to_owned());
        BTreeMap::new()
    } else if path.is_file() {
        match read_index(&path, &mut status) {
            Ok(stored) => {
                status.database = IndexDatabase::Present;
                stored
            }
            Err(error) => {
                status.database = IndexDatabase::Unreadable(error.to_string());
                BTreeMap::new()
            }
        }
    } else {
        BTreeMap::new()
    };
    status.indexed = stored.len();

    for (rel, path) in &files {
        let observed = if is_memory_note(rel) {
            memory_file_mtime_secs(path)
        } else {
            file_mtime_secs(path)
        };
        let observed = match observed {
            Ok(mtime) => mtime,
            Err(_) => {
                status.unreadable.insert(rel.clone());
                continue;
            }
        };
        match stored.get(rel) {
            Some(mtime) if *mtime == observed => status.current += 1,
            Some(_) => {
                status.stale.insert(rel.clone());
            }
            None if eligible(path, rel) => {
                status.missing.insert(rel.clone());
            }
            None => status.ineligible += 1,
        }
    }
    for rel in stored.keys() {
        if !files.contains_key(rel) {
            status.orphaned.insert(rel.clone());
        }
    }
    Ok(status)
}

/// Whether the scan would give this discovered path a `files` row.
fn eligible(path: &Path, rel: &str) -> bool {
    if !matches!(
        resolve_content_shape(path, rel),
        ContentResolution::Indexed(_)
    ) {
        return false;
    }
    // An agent-memory note is indexed only once its original is ready; the
    // scan skips an unready one without a row.
    if is_memory_note(rel) {
        return path
            .parent()
            .is_some_and(|segment| segment.join("ready.json").is_file());
    }
    true
}

fn read_index(path: &Path, status: &mut IndexStatus) -> Result<BTreeMap<String, i64>, StoreError> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute_batch("PRAGMA query_only=ON; PRAGMA busy_timeout=5000;")?;
    status.stamp = Some(read_stamp(&conn)?);
    status.build = read_index_build_state(&conn)?.map(|build| build.state);
    status.last_scan = read_last_scan(&conn)?;
    status.memberships_missing = classification_facets_missing(&conn)?;
    status.path_lookup_ready = chunk_path_lookup_ready(&conn)?;
    status.classification = match read_chunk_classification_backfill(&conn)? {
        None => ClassificationProgress::Absent,
        Some(backfill) if backfill.stalled => ClassificationProgress::Stalled {
            path: backfill.stalled_path,
        },
        Some(backfill) if backfill.completed => ClassificationProgress::Complete,
        Some(_) => ClassificationProgress::Incomplete,
    };
    if !crate::db::sqlite_table_exists(&conn, "files")? {
        return Ok(BTreeMap::new());
    }
    load_file_mtimes(&conn)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::PathBuf;

    use rusqlite::Connection;

    use super::*;
    use crate::scan::scan_journal;
    use crate::test_support::reserve_temp_path;

    struct Journal(PathBuf);

    impl Journal {
        fn new(name: &str) -> Self {
            Self(reserve_temp_path(&format!(
                "solstone-core-indexer-store-status-{name}"
            )))
        }

        fn write(&self, rel: &str, text: &str) {
            let path = self.0.join(rel);
            fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
            fs::write(path, text).expect("write file");
        }
    }

    impl Drop for Journal {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    #[test]
    fn status_diffs_the_index_against_the_disk_without_writing() {
        let journal = Journal::new("diff");
        journal.write("chronicle/20260717/talents/flow.md", "# Flow\n\none");
        journal.write("chronicle/20260717/talents/plan.md", "# Plan\n\none");
        journal.write("chronicle/20260717/talents/gone.md", "# Gone\n\none");

        let absent = inspect_index(&journal.0).expect("status before any index");
        assert_eq!(absent.database, IndexDatabase::Absent);
        assert_eq!(absent.missing.len(), 3);
        assert!(
            !db_path(&journal.0).exists(),
            "a status never creates the index"
        );

        scan_journal(&journal.0, true).expect("scan");
        let clean = inspect_index(&journal.0).expect("status after scan");
        assert_eq!(clean.database, IndexDatabase::Present);
        assert_eq!((clean.current, clean.pending()), (3, 0));
        assert!(clean.last_scan.as_ref().is_some_and(|scan| scan.full));

        // An index row older than its file, a new eligible file, a removed file,
        // and a discovered file the scan skips (an unusable shape sidecar).
        Connection::open(db_path(&journal.0))
            .expect("open for seeding")
            .execute(
                "UPDATE files SET mtime=0 WHERE path='20260717/talents/flow.md'",
                [],
            )
            .expect("age one row");
        journal.write("chronicle/20260717/talents/new.md", "# New\n\none");
        fs::remove_file(journal.0.join("chronicle/20260717/talents/gone.md")).expect("remove");
        journal.write(
            "chronicle/20260717/default/090000_300/talents/odd.md",
            "# Odd",
        );
        journal.write(
            "chronicle/20260717/default/090000_300/talents/shape.json",
            "not json",
        );

        let before = fs::read(db_path(&journal.0)).expect("index bytes");
        let status = inspect_index(&journal.0).expect("status");
        assert_eq!(fs::read(db_path(&journal.0)).expect("index bytes"), before);

        assert_eq!(
            status.stale.iter().collect::<Vec<_>>(),
            ["20260717/talents/flow.md"]
        );
        assert_eq!(
            status.missing.iter().collect::<Vec<_>>(),
            ["20260717/talents/new.md"]
        );
        assert_eq!(
            status.orphaned.iter().collect::<Vec<_>>(),
            ["20260717/talents/gone.md"]
        );
        assert_eq!(status.ineligible, 1);
        assert_eq!(status.current, 1);
        let json = status.to_json_value();
        assert_eq!(json["files"]["stale"], 1);
        assert_eq!(json["generation"]["generation"], 1);
    }

    #[test]
    fn a_file_that_is_not_a_database_is_reported_unreadable_and_left_alone() {
        let journal = Journal::new("not-a-database");
        journal.write("indexer/journal.sqlite", "not a database");
        let status = inspect_index(&journal.0).expect("status");
        assert!(matches!(status.database, IndexDatabase::Unreadable(_)));
        let mut names = fs::read_dir(journal.0.join("indexer"))
            .expect("index dir")
            .map(|entry| entry.expect("entry").file_name())
            .collect::<Vec<_>>();
        names.sort();
        assert_eq!(names, ["journal.sqlite"]);
    }
}
