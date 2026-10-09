// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::{Deserialize, Serialize};
use solstone_core_indexer_store::StoreError;
use solstone_core_indexer_store::db::{
    IndexBuildLifecycle, IndexBuildState, open_index_reader, read_chunk_classification_backfill,
    read_index_build_state, read_segment_aggregate_migration,
};
use solstone_core_system_health::{
    IndexHealth, IndexHealthState, IndexingAttemptObservation, IndexingDiagnostics,
    evaluate_index_health_recent, read_indexing_observations,
};

pub trait IndexMetadata: Send + Sync {
    fn modified(&self, path: &Path) -> std::io::Result<SystemTime>;
}

pub struct FsIndexMetadata;

impl IndexMetadata for FsIndexMetadata {
    fn modified(&self, path: &Path) -> std::io::Result<SystemTime> {
        fs::metadata(path)?.modified()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchClassification {
    pub backfill: String,
    pub stalled_path: Option<String>,
    pub index_build: String,
    pub segment_aggregate: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchIndexHealth {
    /// `complete` when nothing on disk is waiting for the index, `partial` when
    /// something is, `unknown` when the index could not be measured.
    pub coverage: String,
    pub index_activity_at_ms: Option<i64>,
    /// The index health value: `ok`, `behind`, `failing` or `building`.
    pub state: String,
    /// Any failed update recorded in the window, repaired since or not.
    pub observed_failure: bool,
    pub text: String,
    pub index: IndexHealth,
    pub outcomes: Vec<IndexingAttemptObservation>,
    pub classification: SearchClassification,
    pub diagnostics: IndexingDiagnostics,
}

pub fn evaluate_search_index(
    journal_root: &Path,
    metadata_source: &dyn IndexMetadata,
    now: DateTime<Utc>,
) -> SearchIndexHealth {
    let observations = read_indexing_observations(journal_root, now);
    let observed_failure = observations
        .outcomes
        .iter()
        .any(|o| matches!(o.outcome.as_str(), "failed" | "declined" | "ambiguous"));

    let sqlite_path = journal_root.join("indexer/journal.sqlite");
    let wal_path = journal_root.join("indexer/journal.sqlite-wal");

    let sqlite_mtime = metadata_source.modified(&sqlite_path).ok();
    let index_activity_at_ms = if let Some(sqlite_mt) = sqlite_mtime {
        let (wal_mtime, wal_had_non_notfound_error) = match metadata_source.modified(&wal_path) {
            Ok(mtime) => (Some(mtime), false),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => (None, false),
            Err(_) => (None, true),
        };

        if wal_had_non_notfound_error {
            None
        } else {
            let newest_mtime = match wal_mtime {
                Some(wal) => sqlite_mt.max(wal),
                None => sqlite_mt,
            };
            let newest_dt = newest_mtime
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|d| Utc.timestamp_millis_opt(d.as_millis() as i64).single());

            match newest_dt {
                Some(dt) if dt <= now + Duration::seconds(300) => Some(dt.timestamp_millis()),
                _ => None,
            }
        }
    } else {
        None
    };

    // Measured after the activity time is read: opening the index, even
    // read-only, can create an empty write-ahead file beside it.
    let index = evaluate_index_health_recent(journal_root, &observations);

    let mut backfill = "unknown".to_owned();
    let mut stalled_path = None;
    let mut index_build = "unknown".to_owned();
    let mut segment_aggregate = "unknown".to_owned();

    match open_index_reader(journal_root) {
        Ok(conn) => {
            match read_chunk_classification_backfill(&conn) {
                Ok(Some(b)) => {
                    if b.stalled {
                        backfill = "stalled".to_owned();
                        stalled_path = b.stalled_path;
                    } else if !b.completed {
                        backfill = "incomplete".to_owned();
                    } else {
                        backfill = "exhausted".to_owned();
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    log::warn!("failed to read chunk classification backfill: {err}");
                }
            }

            match read_index_build_state(&conn) {
                Ok(Some(IndexBuildState {
                    state: IndexBuildLifecycle::Building,
                    ..
                })) => {
                    index_build = "building".to_owned();
                }
                Ok(Some(IndexBuildState {
                    state: IndexBuildLifecycle::Complete,
                    ..
                })) => {
                    index_build = "complete".to_owned();
                }
                Ok(None) => {}
                Err(err) => {
                    log::warn!("failed to read index build state: {err}");
                }
            }

            match read_segment_aggregate_migration(&conn) {
                Ok(Some(mig)) => {
                    if mig.completed {
                        segment_aggregate = "complete".to_owned();
                    } else {
                        segment_aggregate = "incomplete".to_owned();
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    log::warn!("failed to read segment aggregate migration: {err}");
                }
            }
        }
        Err(StoreError::MissingFile(_)) => {
            // Missing file is normal, no warning
        }
        Err(err) => {
            log::warn!("failed to open index reader: {err}");
        }
    }

    let classification = SearchClassification {
        backfill,
        stalled_path,
        index_build,
        segment_aggregate,
    };

    let coverage = match index.failure {
        Some(solstone_core_system_health::IndexFailure::Unreadable) => "unknown",
        _ if index.pending == 0 && index.state != IndexHealthState::Building => "complete",
        _ => "partial",
    };

    SearchIndexHealth {
        coverage: coverage.to_owned(),
        index_activity_at_ms,
        state: index.state.as_str().to_owned(),
        observed_failure,
        text: index.text.clone(),
        index,
        outcomes: observations.outcomes,
        classification,
        diagnostics: observations.diagnostics,
    }
}

#[cfg(all(test, feature = "full-tests"))]
mod full_tests {
    use super::*;
    use solstone_core_indexer_store::db::{
        ChunkClassificationBackfill, open_index, write_chunk_classification_backfill,
        write_segment_aggregate_migration,
    };
    use std::fs;
    use tempfile::tempdir;

    fn setup_utc_journal(journal: &Path) {
        let config_dir = journal.join("config");
        fs::create_dir_all(&config_dir).unwrap();
        fs::write(
            config_dir.join("journal.json"),
            r#"{"identity":{"timezone":"UTC"}}"#,
        )
        .unwrap();
    }

    #[test]
    fn full_tests_classification_and_real_sqlite() {
        let dir = tempdir().unwrap();
        setup_utc_journal(dir.path());
        let now = Utc::now();

        // 1. Missing database (no file) stays the first case
        let health_absent = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_absent.coverage, "complete");
        assert_eq!(health_absent.state, "ok");
        assert_eq!(health_absent.classification.backfill, "unknown");
        assert_eq!(health_absent.index_activity_at_ms, None);

        // 2. Absent tables: open_index in test setup only, then DROP TABLE classification tables
        let conn = open_index(dir.path()).unwrap();
        conn.execute_batch(
            "
            DROP TABLE IF EXISTS chunk_classification;
            DROP TABLE IF EXISTS chunk_classification_facets;
            DROP TABLE IF EXISTS chunk_classification_backfill;
            ",
        )
        .unwrap();
        let mut stmt = conn
            .prepare("SELECT name FROM sqlite_master ORDER BY name")
            .unwrap();
        let master_names_before: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        drop(stmt);
        drop(conn);

        let health_dropped = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_dropped.classification.backfill, "unknown");

        let conn_check = open_index_reader(dir.path()).unwrap();
        let mut stmt = conn_check
            .prepare("SELECT name FROM sqlite_master ORDER BY name")
            .unwrap();
        let master_names_after: Vec<String> = stmt
            .query_map([], |row| row.get(0))
            .unwrap()
            .map(Result::unwrap)
            .collect();
        assert_eq!(master_names_before, master_names_after);
        drop(stmt);
        drop(conn_check);

        // 3. open_index again to recreate schema
        let conn = open_index(dir.path()).unwrap();
        let mtime_secs = (now - Duration::seconds(5)).timestamp();
        let mtime_ms = mtime_secs * 1000;
        let db_path = dir.path().join("indexer/journal.sqlite");
        let wal_path = dir.path().join("indexer/journal.sqlite-wal");
        let set_mtimes = || {
            filetime::set_file_mtime(&db_path, filetime::FileTime::from_unix_time(mtime_secs, 0))
                .unwrap();
            if wal_path.exists() {
                filetime::set_file_mtime(
                    &wal_path,
                    filetime::FileTime::from_unix_time(mtime_secs, 0),
                )
                .unwrap();
            }
        };

        // Stalled (stalled: true, completed: false): state failing, nothing pending on disk,
        // index_activity_at_ms equals that mtime, observed_failure false, stalled_path preserved
        write_chunk_classification_backfill(
            &conn,
            &ChunkClassificationBackfill {
                cursor: "10".to_owned(),
                completed: false,
                stalled: true,
                stalled_path: Some("20261005/talents/stalled.md".to_owned()),
                resume_count: 0,
            },
        )
        .unwrap();
        drop(conn);
        set_mtimes();

        let health_stalled = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_stalled.coverage, "complete");
        assert_eq!(health_stalled.state, "failing");
        assert!(!health_stalled.observed_failure);
        assert_eq!(health_stalled.classification.backfill, "stalled");
        assert_eq!(
            health_stalled.classification.stalled_path.as_deref(),
            Some("20261005/talents/stalled.md")
        );
        assert_eq!(health_stalled.index_activity_at_ms, Some(mtime_ms));

        // Separately stalled: false, completed: false: backfill incomplete, state behind, activity mtime still set
        let conn = open_index(dir.path()).unwrap();
        write_chunk_classification_backfill(
            &conn,
            &ChunkClassificationBackfill {
                cursor: "50".to_owned(),
                completed: false,
                stalled: false,
                stalled_path: None,
                resume_count: 0,
            },
        )
        .unwrap();
        drop(conn);
        set_mtimes();

        let health_inprog = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_inprog.coverage, "complete");
        assert_eq!(health_inprog.state, "behind");
        assert_eq!(health_inprog.classification.backfill, "incomplete");
        assert_eq!(health_inprog.index_activity_at_ms, Some(mtime_ms));

        // completed: true, stalled: false, plus index_build complete and segment_aggregate completed:
        // nothing pending, state ok, backfill exhausted
        let conn2 = open_index(dir.path()).unwrap();
        write_chunk_classification_backfill(
            &conn2,
            &ChunkClassificationBackfill {
                cursor: "100".to_owned(),
                completed: true,
                stalled: false,
                stalled_path: None,
                resume_count: 0,
            },
        )
        .unwrap();
        conn2.execute(
            "REPLACE INTO index_build_state(id, schema_version, state, files_count, chunks_count) VALUES (1, 1, 'complete', 10, 10)",
            [],
        )
        .unwrap();
        write_segment_aggregate_migration(&conn2, "100", true).unwrap();
        drop(conn2);
        set_mtimes();

        let health_complete = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_complete.coverage, "complete");
        assert_eq!(health_complete.state, "ok");
        assert!(!health_complete.observed_failure);
        assert_eq!(health_complete.classification.backfill, "exhausted");
        assert_eq!(health_complete.classification.index_build, "complete");
        assert_eq!(health_complete.classification.segment_aggregate, "complete");

        // Then a think health index.attempt failure with ts inside the window, for a
        // file that is not on disk: observed, but nothing is behind, so still ok.
        let today = now.format("%Y%m%d").to_string();
        let health_dir = dir.path().join("chronicle").join(&today).join("health");
        fs::create_dir_all(&health_dir).unwrap();
        fs::write(
            health_dir.join("think.jsonl"),
            format!(
                "{}\n",
                serde_json::json!({
                    "event": "index.attempt",
                    "ts": now.timestamp_millis() - 1000,
                    "path": format!("{today}/note.md"),
                    "outcome": "failed"
                })
            ),
        )
        .unwrap();

        set_mtimes();
        // Every reader this page uses leaves the index file's bytes unchanged.
        let before = fs::read(&db_path).unwrap();
        let health_observed = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(fs::read(&db_path).unwrap(), before);
        assert!(fs::metadata(&wal_path).map_or(true, |wal| wal.len() == 0));
        assert_eq!(health_observed.state, "ok");
        assert!(health_observed.observed_failure);
        assert_eq!(health_observed.index_activity_at_ms, Some(mtime_ms));
    }
}
