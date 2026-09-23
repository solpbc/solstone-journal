// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fs;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use chrono::{DateTime, Duration, TimeZone, Utc};
use serde::Serialize;
use solstone_core_system_health::{IndexerPhase, SummaryFreshness};

pub const SEARCH_TEXT_CURRENT: &str = "search is current.";
pub const SEARCH_TEXT_BEHIND_7_DAYS: &str = "search is more than 7 days behind.";
pub const SEARCH_TEXT_BEHIND_ATTEMPT_FAILED: &str =
    "search is behind; the latest indexer attempt failed.";
pub const SEARCH_TEXT_UNCLEAR: &str = "it's unclear whether search is current.";
pub const SEARCH_NOTE_ATTEMPT_FAILED: &str =
    "the latest indexer attempt failed; search-backed consumers may be stale.";

pub trait IndexMetadata: Send + Sync {
    fn modified(&self, path: &Path) -> std::io::Result<SystemTime>;
}

pub struct FsIndexMetadata;

impl IndexMetadata for FsIndexMetadata {
    fn modified(&self, path: &Path) -> std::io::Result<SystemTime> {
        fs::metadata(path)?.modified()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SearchFreshnessEvaluation {
    pub state: String,
    pub updated_at_ms: Option<i64>,
    pub last_attempt_failed: bool,
    pub text: String,
}

pub fn evaluate_search_freshness(
    journal_root: &Path,
    metadata_source: &dyn IndexMetadata,
    indexer_phase: Option<&IndexerPhase>,
    summary_freshness: SummaryFreshness,
    now: DateTime<Utc>,
) -> SearchFreshnessEvaluation {
    let sqlite_path = journal_root.join("indexer/journal.sqlite");
    let wal_path = journal_root.join("indexer/journal.sqlite-wal");

    let sqlite_mtime = match metadata_source.modified(&sqlite_path) {
        Ok(mtime) => mtime,
        Err(_) => {
            // NotFound or other metadata error on sqlite -> unknown, flag false, updated_at_ms null (ignore WAL)
            return SearchFreshnessEvaluation {
                state: "unknown".to_owned(),
                updated_at_ms: None,
                last_attempt_failed: false,
                text: SEARCH_TEXT_UNCLEAR.to_owned(),
            };
        }
    };

    let wal_mtime = match metadata_source.modified(&wal_path) {
        Ok(mtime) => Some(mtime),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(_) => {
            // Other metadata error on existing WAL -> unknown, flag false, do not return Err
            return SearchFreshnessEvaluation {
                state: "unknown".to_owned(),
                updated_at_ms: None,
                last_attempt_failed: false,
                text: SEARCH_TEXT_UNCLEAR.to_owned(),
            };
        }
    };

    let newest_mtime = match wal_mtime {
        Some(wal) => sqlite_mtime.max(wal),
        None => sqlite_mtime,
    };

    let updated_at_ms = newest_mtime
        .duration_since(UNIX_EPOCH)
        .ok()
        .map(|d| d.as_millis() as i64);

    let newest_dt = match newest_mtime.duration_since(UNIX_EPOCH) {
        Ok(d) => Utc.timestamp_millis_opt(d.as_millis() as i64).single(),
        Err(_) => None,
    };

    let Some(newest_dt) = newest_dt else {
        return SearchFreshnessEvaluation {
            state: "unknown".to_owned(),
            updated_at_ms,
            last_attempt_failed: false,
            text: SEARCH_TEXT_UNCLEAR.to_owned(),
        };
    };

    // Newest mtime more than 5 minutes ahead of now -> unknown
    if newest_dt > now + Duration::seconds(300) {
        return SearchFreshnessEvaluation {
            state: "unknown".to_owned(),
            updated_at_ms,
            last_attempt_failed: false,
            text: SEARCH_TEXT_UNCLEAR.to_owned(),
        };
    }

    // Age strictly over 7 days -> stale, flag false (wins over summary and attempt; exactly 7 days is not this arm)
    if now - newest_dt > Duration::days(7) {
        return SearchFreshnessEvaluation {
            state: "stale".to_owned(),
            updated_at_ms,
            last_attempt_failed: false,
            text: SEARCH_TEXT_BEHIND_7_DAYS.to_owned(),
        };
    }

    // Summary missing, unreadable, degraded, or not Fresh -> unknown, flag false
    if summary_freshness != SummaryFreshness::Fresh {
        return SearchFreshnessEvaluation {
            state: "unknown".to_owned(),
            updated_at_ms,
            last_attempt_failed: false,
            text: SEARCH_TEXT_UNCLEAR.to_owned(),
        };
    }

    // indexer_phase.success == false -> stale, flag true
    if indexer_phase.is_some_and(|p| !p.success) {
        return SearchFreshnessEvaluation {
            state: "stale".to_owned(),
            updated_at_ms,
            last_attempt_failed: true,
            text: SEARCH_TEXT_BEHIND_ATTEMPT_FAILED.to_owned(),
        };
    }

    // Else fresh
    SearchFreshnessEvaluation {
        state: "fresh".to_owned(),
        updated_at_ms,
        last_attempt_failed: false,
        text: SEARCH_TEXT_CURRENT.to_owned(),
    }
}
