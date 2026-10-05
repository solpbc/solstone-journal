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
    IndexingAttemptObservation, IndexingDiagnostics, read_indexing_observations,
};

pub const SEARCH_TEXT_BEHIND_ATTEMPT_FAILED: &str =
    "search couldn't update on its last try, so recent moments may not turn up in search yet.";
pub const SEARCH_TEXT_UNCLEAR: &str = "it's unclear when search last caught up.";
pub const SEARCH_NOTE_ATTEMPT_FAILED: &str =
    "the latest indexer attempt failed; search-backed consumers may be stale.";

/// Fills a search template's `{age}` with the time since the index's last write.
pub fn render_search_text(template: &str, age: Duration) -> String {
    template.replace(
        "{age}",
        &solstone_core_system_health::format_summary_age(age),
    )
}

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
    pub coverage: String,
    pub index_activity_at_ms: Option<i64>,
    pub state: String,
    pub observed_failure: bool,
    pub text: String,
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
        let wal_result = metadata_source.modified(&wal_path);
        let wal_mtime = match wal_result {
            Ok(mtime) => Some(mtime),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => None,
            Err(_) => {
                // WAL error other than NotFound -> unreadable activity
                None
            }
        };

        let wal_had_non_notfound_error = matches!(
            metadata_source.modified(&wal_path),
            Err(e) if e.kind() != std::io::ErrorKind::NotFound
        );

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

    let (state, text) = if observed_failure {
        (
            "degraded".to_owned(),
            SEARCH_TEXT_BEHIND_ATTEMPT_FAILED.to_owned(),
        )
    } else if classification.backfill == "stalled" || classification.backfill == "incomplete" {
        (
            "incomplete".to_owned(),
            "search classification is still incomplete.".to_owned(),
        )
    } else {
        let text = if let Some(ms) = index_activity_at_ms {
            let dt = Utc.timestamp_millis_opt(ms).single().unwrap_or(now);
            render_search_text("search index last changed {age} ago.", now - dt)
        } else {
            SEARCH_TEXT_UNCLEAR.to_owned()
        };
        ("unknown".to_owned(), text)
    };

    SearchIndexHealth {
        coverage: "unknown".to_owned(),
        index_activity_at_ms,
        state,
        observed_failure,
        text,
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
        assert_eq!(health_absent.coverage, "unknown");
        assert_eq!(health_absent.state, "unknown");
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
        let mut conn = open_index(dir.path()).unwrap();
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

        // Stalled (stalled: true, completed: false): state incomplete, coverage unknown,
        // index_activity_at_ms equals that mtime, observed_failure false, stalled_path preserved
        write_chunk_classification_backfill(
            &mut conn,
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
        assert_eq!(health_stalled.coverage, "unknown");
        assert_eq!(health_stalled.state, "incomplete");
        assert_eq!(health_stalled.observed_failure, false);
        assert_eq!(health_stalled.classification.backfill, "stalled");
        assert_eq!(
            health_stalled.classification.stalled_path.as_deref(),
            Some("20261005/talents/stalled.md")
        );
        assert_eq!(health_stalled.index_activity_at_ms, Some(mtime_ms));

        // Separately stalled: false, completed: false: backfill incomplete, state incomplete, activity mtime still set
        let mut conn = open_index(dir.path()).unwrap();
        write_chunk_classification_backfill(
            &mut conn,
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
        assert_eq!(health_inprog.coverage, "unknown");
        assert_eq!(health_inprog.state, "incomplete");
        assert_eq!(health_inprog.classification.backfill, "incomplete");
        assert_eq!(health_inprog.index_activity_at_ms, Some(mtime_ms));

        // completed: true, stalled: false, plus index_build complete and segment_aggregate completed:
        // coverage unknown, state is not incomplete and not degraded, backfill exhausted
        let mut conn2 = open_index(dir.path()).unwrap();
        write_chunk_classification_backfill(
            &mut conn2,
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
        write_segment_aggregate_migration(&mut conn2, "100", true).unwrap();
        drop(conn2);
        set_mtimes();

        let health_complete = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_complete.coverage, "unknown");
        assert_ne!(health_complete.state, "incomplete");
        assert_ne!(health_complete.state, "degraded");
        assert_eq!(health_complete.state, "unknown");
        assert_eq!(health_complete.observed_failure, false);
        assert_eq!(health_complete.classification.backfill, "exhausted");
        assert_eq!(health_complete.classification.index_build, "complete");
        assert_eq!(health_complete.classification.segment_aggregate, "complete");

        // Then a think health index.attempt failure with ts inside the window: state degraded, observed_failure true, activity mtime still set
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
        let health_degraded = evaluate_search_index(dir.path(), &FsIndexMetadata, now);
        assert_eq!(health_degraded.state, "degraded");
        assert_eq!(health_degraded.observed_failure, true);
        assert_eq!(health_degraded.index_activity_at_ms, Some(mtime_ms));
    }
}
