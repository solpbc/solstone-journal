// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Index artifacts that a completed journal-layout migration invalidates.
//!
//! When day directories move, every indexed path in the SQLite database names a
//! location that no longer exists. Deleting the database is the correct repair
//! because the index is derived state: the next scan rebuilds it from the moved
//! journal. Nothing here reads or writes journal source data.

use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

use crate::StoreError;
use crate::db::{db_path, reset_index};
use crate::scan::scan_journal;

const EXPECTED_CHUNK_COLUMNS: [&str; 7] =
    ["content", "path", "day", "facet", "agent", "stream", "idx"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum IndexStreamMigration {
    Absent,
    Current,
    Rebuilt { missing: Vec<String> },
    WouldRebuild { missing: Vec<String> },
}

/// Detect the pre-stream FTS schema and rebuild the derived index through the
/// index owner's reset/full-scan path. Missing databases, missing tables, and
/// unreadable schema probes are the reference no-op.
pub fn migrate_index_stream(
    journal: &Path,
    dry_run: bool,
) -> Result<IndexStreamMigration, StoreError> {
    let path = db_path(journal);
    if !path.exists() {
        return Ok(IndexStreamMigration::Absent);
    }
    let columns = match Connection::open_with_flags(&path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .and_then(|connection| {
            let mut statement = connection.prepare("PRAGMA table_info(chunks)")?;
            statement
                .query_map([], |row| row.get::<_, String>(1))?
                .collect::<Result<Vec<_>, _>>()
        }) {
        Ok(columns) if !columns.is_empty() => columns,
        Ok(_) | Err(_) => return Ok(IndexStreamMigration::Absent),
    };
    let missing = EXPECTED_CHUNK_COLUMNS
        .iter()
        .filter(|column| !columns.iter().any(|actual| actual == **column))
        .map(|column| (*column).to_owned())
        .collect::<Vec<_>>();
    if missing.is_empty() {
        return Ok(IndexStreamMigration::Current);
    }
    if dry_run {
        return Ok(IndexStreamMigration::WouldRebuild { missing });
    }
    reset_index(journal)?;
    scan_journal(journal, true)?;
    Ok(IndexStreamMigration::Rebuilt { missing })
}

/// Which legacy index artifacts one removal pass unlinked.
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct IndexArtifactRemovalReport {
    /// Journal-relative paths removed by this pass, in deletion order.
    pub removed: Vec<String>,
}

impl IndexArtifactRemovalReport {
    /// How many artifacts this pass unlinked.
    pub fn deleted(&self) -> usize {
        self.removed.len()
    }
}

/// Delete the index database and its write-ahead sidecars, then prove they are
/// gone.
///
/// The absence check is not decoration: a silent failure here leaves a database
/// full of pre-migration paths that the next reader would treat as current.
pub fn remove_legacy_index_artifacts(
    journal: &Path,
) -> Result<IndexArtifactRemovalReport, StoreError> {
    let mut report = IndexArtifactRemovalReport::default();
    for path in legacy_index_artifacts(journal) {
        if !path.exists() {
            continue;
        }
        fs::remove_file(&path)?;
        report
            .removed
            .push(relative_label(journal, &path).into_owned());
    }
    let surviving = legacy_index_artifacts(journal)
        .into_iter()
        .filter(|path| path.exists())
        .map(|path| relative_label(journal, &path).into_owned())
        .collect::<Vec<_>>();
    if !surviving.is_empty() {
        return Err(StoreError::EdgeFileFailed(format!(
            "index files remain after migration: {}",
            surviving.join(", ")
        )));
    }
    Ok(report)
}

/// The database plus the two SQLite WAL sidecars that shadow it.
fn legacy_index_artifacts(journal: &Path) -> Vec<PathBuf> {
    let database = db_path(journal);
    let name = database
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let directory = database.parent().map(Path::to_path_buf).unwrap_or_default();
    vec![
        database.clone(),
        directory.join(format!("{name}-wal")),
        directory.join(format!("{name}-shm")),
    ]
}

fn relative_label<'a>(journal: &Path, path: &'a Path) -> std::borrow::Cow<'a, str> {
    path.strip_prefix(journal).unwrap_or(path).to_string_lossy()
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;

    /// Match the crate's existing test convention: a stamped temp root, no
    /// added dev-dependency.
    fn temp_root(name: &str) -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time should be available")
            .as_nanos();
        std::env::temp_dir().join(format!(
            "solstone-core-indexer-store-migrations-{name}-{stamp}"
        ))
    }

    fn journal_with_index(name: &str, files: &[&str]) -> PathBuf {
        let root = temp_root(name);
        let index = root.join("indexer");
        fs::create_dir_all(&index).expect("index directory");
        for artifact in files {
            fs::write(index.join(artifact), b"bytes").expect("artifact written");
        }
        root
    }

    #[test]
    fn the_database_and_both_wal_sidecars_are_removed_together() {
        let root = journal_with_index(
            "all-three",
            &["journal.sqlite", "journal.sqlite-wal", "journal.sqlite-shm"],
        );

        let report = remove_legacy_index_artifacts(&root).expect("removal runs");

        assert_eq!(report.deleted(), 3);
        assert_eq!(
            report.removed,
            [
                "indexer/journal.sqlite",
                "indexer/journal.sqlite-wal",
                "indexer/journal.sqlite-shm"
            ]
        );
        for artifact in ["journal.sqlite", "journal.sqlite-wal", "journal.sqlite-shm"] {
            assert!(!root.join("indexer").join(artifact).exists());
        }
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn a_partial_set_removes_only_what_is_present() {
        let root = journal_with_index("partial", &["journal.sqlite"]);

        let report = remove_legacy_index_artifacts(&root).expect("removal runs");

        assert_eq!(report.removed, ["indexer/journal.sqlite"]);
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn an_absent_index_is_a_no_op_rather_than_a_failure() {
        let root = temp_root("absent");

        let report = remove_legacy_index_artifacts(&root).expect("removal runs");

        assert_eq!(report, IndexArtifactRemovalReport::default());
        assert_eq!(report.deleted(), 0);
    }

    #[test]
    fn removal_is_idempotent_across_repeated_runs() {
        let root = journal_with_index("idempotent", &["journal.sqlite", "journal.sqlite-wal"]);

        assert_eq!(
            remove_legacy_index_artifacts(&root)
                .expect("first run")
                .deleted(),
            2
        );
        assert_eq!(
            remove_legacy_index_artifacts(&root)
                .expect("second run")
                .deleted(),
            0
        );
        fs::remove_dir_all(root).expect("remove test root");
    }

    #[test]
    fn legacy_chunk_schema_is_detected_then_rebuilt_through_the_index_owner() {
        let root = temp_root("stream-schema");
        fs::create_dir_all(root.join("indexer")).unwrap();
        let connection = Connection::open(db_path(&root)).unwrap();
        connection
            .execute_batch(
                "CREATE VIRTUAL TABLE chunks USING fts5(content, path UNINDEXED, day UNINDEXED, facet UNINDEXED, agent UNINDEXED, idx UNINDEXED);",
            )
            .unwrap();
        drop(connection);

        assert_eq!(
            migrate_index_stream(&root, true).unwrap(),
            IndexStreamMigration::WouldRebuild {
                missing: vec!["stream".to_owned()]
            }
        );
        assert_eq!(
            migrate_index_stream(&root, false).unwrap(),
            IndexStreamMigration::Rebuilt {
                missing: vec!["stream".to_owned()]
            }
        );
        assert_eq!(
            migrate_index_stream(&root, false).unwrap(),
            IndexStreamMigration::Current
        );
        fs::remove_dir_all(root).unwrap();
    }
}
