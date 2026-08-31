// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::BTreeMap;
use std::fmt;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use rusqlite::{Connection, OpenFlags};

use crate::{
    DatabaseSignatureError, TrendsSignature, health_dedupe_database_path, read_database_signature,
};

type StatsCache = BTreeMap<String, (TrendsSignature, Arc<HealthDedupeStats>)>;

static HEALTH_DEDUPE_STATS_CACHE: OnceLock<Mutex<StatsCache>> = OnceLock::new();

/// Folded, read-only aggregate statistics for `health_dedupe`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthDedupeStats {
    /// All aggregate rows.
    pub total: u64,
    /// Row counts keyed by record type.
    pub by_type: BTreeMap<String, u64>,
    /// Row counts keyed by source family.
    pub by_source: BTreeMap<String, u64>,
    /// Row counts keyed by `YYYY-MM` derived from aggregate `start_time`.
    pub by_month: BTreeMap<String, u64>,
    /// Row counts keyed by `YYYYMMDD` derived from aggregate `start_time`.
    pub by_day: BTreeMap<String, u64>,
    /// Per-record-type first and last aggregate timestamps.
    pub type_ranges: BTreeMap<String, HealthDedupeTimeRange>,
    /// First and last timestamp across the entire aggregate.
    pub coverage_window: HealthDedupeTimeRange,
}

/// A first/last timestamp pair from the aggregate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HealthDedupeTimeRange {
    /// Earliest timestamp, if any.
    pub first: Option<String>,
    /// Latest timestamp, if any.
    pub last: Option<String>,
}

/// Aggregate reader failures for a present database.
#[derive(Debug)]
pub enum HealthDedupeStatsError {
    /// Signature metadata could not be read.
    Io { path: PathBuf, source: io::Error },
    /// SQLite could not open or query the aggregate read-only.
    Sqlite(rusqlite::Error),
    /// The process cache lock was poisoned.
    CachePoisoned,
    /// SQLite returned a negative count.
    NegativeCount(i64),
}

impl fmt::Display for HealthDedupeStatsError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "could not stat {}: {source}", path.display())
            }
            Self::Sqlite(source) => write!(
                formatter,
                "could not read health dedupe aggregate: {source}"
            ),
            Self::CachePoisoned => {
                formatter.write_str("health dedupe stats cache lock was poisoned")
            }
            Self::NegativeCount(count) => {
                write!(formatter, "aggregate returned negative count {count}")
            }
        }
    }
}

impl std::error::Error for HealthDedupeStatsError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Sqlite(source) => Some(source),
            Self::CachePoisoned | Self::NegativeCount(_) => None,
        }
    }
}

/// Reads a cached aggregate fold; absence is `Ok(None)`, never a zero-filled fold.
///
/// The cache includes WAL metadata for Python parity. Native rebuilds remove WAL,
/// SHM, and rollback-journal sidecars before and after publication, so its WAL
/// components are normally inert for a natively written store.
pub fn read_health_dedupe_stats(
    journal_root: impl AsRef<Path>,
) -> Result<Option<Arc<HealthDedupeStats>>, HealthDedupeStatsError> {
    let db_path = health_dedupe_database_path(journal_root.as_ref());
    let Some(signature) = read_database_signature(&db_path).map_err(signature_error)? else {
        return Ok(None);
    };
    let cache_key = db_path.to_string_lossy().into_owned();
    let cache = HEALTH_DEDUPE_STATS_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut cache = cache
        .lock()
        .map_err(|_| HealthDedupeStatsError::CachePoisoned)?;
    if let Some((cached_signature, stats)) = cache.get(&cache_key)
        && *cached_signature == signature
    {
        return Ok(Some(Arc::clone(stats)));
    }
    let stats = Arc::new(fold_stats(&db_path)?);
    cache.insert(cache_key, (signature, Arc::clone(&stats)));
    Ok(Some(stats))
}

fn signature_error(error: DatabaseSignatureError) -> HealthDedupeStatsError {
    match error {
        DatabaseSignatureError::Io { path, source } => HealthDedupeStatsError::Io { path, source },
    }
}

fn fold_stats(path: &Path) -> Result<HealthDedupeStats, HealthDedupeStatsError> {
    let uri = format!("file:{}?mode=ro", path.to_string_lossy());
    let connection = Connection::open_with_flags(
        uri,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(HealthDedupeStatsError::Sqlite)?;
    connection
        .execute_batch("PRAGMA temp_store = MEMORY")
        .map_err(HealthDedupeStatsError::Sqlite)?;
    let (window_start, window_end): (Option<String>, Option<String>) = connection
        .query_row(
            "SELECT MIN(start_time) AS s, MAX(start_time) AS e FROM health_dedupe",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(HealthDedupeStatsError::Sqlite)?;
    let mut statement = connection
        .prepare(
            "SELECT\n                record_type,\n                source_family,\n                replace(substr(start_time, 1, 10), '-', '') AS d,\n                COUNT(*) AS n,\n                MIN(start_time) AS min_start,\n                MAX(start_time) AS max_start\n            FROM health_dedupe\n            GROUP BY record_type, source_family, d",
        )
        .map_err(HealthDedupeStatsError::Sqlite)?;
    let grouped = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<String>>(4)?,
                row.get::<_, Option<String>>(5)?,
            ))
        })
        .map_err(HealthDedupeStatsError::Sqlite)?;
    let mut stats = HealthDedupeStats {
        total: 0,
        by_type: BTreeMap::new(),
        by_source: BTreeMap::new(),
        by_month: BTreeMap::new(),
        by_day: BTreeMap::new(),
        type_ranges: BTreeMap::new(),
        coverage_window: HealthDedupeTimeRange {
            first: window_start,
            last: window_end,
        },
    };
    for group in grouped {
        let (record_type, source_family, day, count, first, last) =
            group.map_err(HealthDedupeStatsError::Sqlite)?;
        let count =
            u64::try_from(count).map_err(|_| HealthDedupeStatsError::NegativeCount(count))?;
        stats.total += count;
        *stats.by_type.entry(record_type.clone()).or_default() += count;
        *stats.by_source.entry(source_family).or_default() += count;
        *stats.by_day.entry(day.clone()).or_default() += count;
        if day.len() >= 6 {
            *stats
                .by_month
                .entry(format!("{}-{}", &day[..4], &day[4..6]))
                .or_default() += count;
        }
        let range = stats
            .type_ranges
            .entry(record_type)
            .or_insert(HealthDedupeTimeRange {
                first: None,
                last: None,
            });
        if first
            .as_ref()
            .is_some_and(|value| range.first.as_ref().is_none_or(|current| value < current))
        {
            range.first = first;
        }
        if last
            .as_ref()
            .is_some_and(|value| range.last.as_ref().is_none_or(|current| value > current))
        {
            range.last = last;
        }
    }
    Ok(stats)
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;

    use super::*;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-aggregate-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir_all(path.join("imports")).unwrap();
            Self(path)
        }

        fn path(&self) -> &Path {
            &self.0
        }
    }

    impl Drop for TempDir {
        fn drop(&mut self) {
            fs::remove_dir_all(&self.0).unwrap();
        }
    }

    fn database(root: &Path, rows: usize) {
        let connection = Connection::open(root.join("imports/health-dedupe.sqlite")).unwrap();
        connection
            .execute_batch(
                "CREATE TABLE health_dedupe (dedupe_key TEXT PRIMARY KEY, source_family TEXT NOT NULL, record_type TEXT NOT NULL, start_time TEXT NOT NULL);",
            )
            .unwrap();
        for index in 0..rows {
            connection
                .execute(
                    "INSERT INTO health_dedupe VALUES (?1, 'apple_health', 'synthetic_type', ?2)",
                    (
                        format!("synthetic-{index}"),
                        format!("2024-01-0{}T00:00:00Z", index + 1),
                    ),
                )
                .unwrap();
        }
    }

    #[test]
    fn cache_identity_changes_when_database_signature_changes() {
        let temporary = TempDir::new();
        HEALTH_DEDUPE_STATS_CACHE
            .get_or_init(|| Mutex::new(BTreeMap::new()))
            .lock()
            .unwrap()
            .clear();
        database(temporary.path(), 1);
        let first = read_health_dedupe_stats(temporary.path()).unwrap().unwrap();
        let second = read_health_dedupe_stats(temporary.path()).unwrap().unwrap();
        assert!(Arc::ptr_eq(&first, &second));
        let connection =
            Connection::open(temporary.path().join("imports/health-dedupe.sqlite")).unwrap();
        connection
            .execute(
                "INSERT INTO health_dedupe VALUES ('synthetic-next', 'apple_health', 'synthetic_type', '2024-01-09T00:00:00Z')",
                [],
            )
            .unwrap();
        drop(connection);
        let changed = read_health_dedupe_stats(temporary.path()).unwrap().unwrap();
        assert_eq!(changed.total, 2);
        assert!(!Arc::ptr_eq(&first, &changed));
        let cache_len = HEALTH_DEDUPE_STATS_CACHE
            .get()
            .unwrap()
            .lock()
            .unwrap()
            .len();
        assert_eq!(cache_len, 1);
    }
}
