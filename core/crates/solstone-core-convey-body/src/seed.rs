// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde_json::{Map, Value};
use sha2::{Digest, Sha256};

use crate::read_health_dedupe_stats;

const SCHEMA: &str = "
CREATE TABLE health_dedupe (
    dedupe_key TEXT PRIMARY KEY,
    source_family TEXT NOT NULL,
    source_record_id TEXT,
    record_type TEXT NOT NULL,
    start_time TEXT NOT NULL,
    end_time TEXT,
    value_hash TEXT,
    first_import_id TEXT,
    last_seen_import_id TEXT,
    normalized_ref TEXT,
    raw_ref TEXT,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL
);
CREATE INDEX idx_health_dedupe_source_record
ON health_dedupe (source_family, source_record_id);
CREATE INDEX idx_health_dedupe_record_time
ON health_dedupe (record_type, start_time, end_time);
";

/// A complete synthetic body-journal fixture specification.
#[derive(Debug, Clone)]
pub struct BodyJournalSeed {
    /// Explicit journal date directories, which need not form a range.
    pub dates: BTreeSet<String>,
    /// Explicit import bundle specifications.
    pub bundles: Vec<BodySeedBundle>,
    /// Whether the health aggregate is directly written.
    pub aggregate: BodyAggregateSeed,
}

/// One synthetic import bundle and its per-shard JSON rows.
#[derive(Debug, Clone)]
pub struct BodySeedBundle {
    /// Bundle directory and default row import ID.
    pub import_id: String,
    /// Bundle source family; non-native values are intentionally allowed.
    pub source_family: String,
    /// Manifest presence and its selected fields.
    pub manifest: BodySeedManifest,
    /// Rows by explicit normalized `YYYY-MM` shard name.
    pub shards: BTreeMap<String, Vec<Map<String, Value>>>,
}

/// Synthetic manifest presence and fields.
#[derive(Debug, Clone)]
pub enum BodySeedManifest {
    /// Do not create a manifest at all.
    Absent,
    /// Create a manifest, with absent `source_type` or `entry_count` represented by `None`.
    Present {
        /// Source family exposed to disk inventory, including unknown values.
        source_type: Option<String>,
        /// Optional positive/zero row-count claim.
        entry_count: Option<u64>,
        /// Additional manifest keys retained verbatim.
        extra: Map<String, Value>,
    },
}

/// Whether the seeder directly writes a synthetic aggregate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyAggregateSeed {
    /// Leave the aggregate absent.
    Absent,
    /// Create the aggregate from seeded normalized rows.
    Direct,
}

/// Description of the synthetic journal state written by the seeder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySeedReport {
    /// Created journal date directories.
    pub dates: BTreeSet<String>,
    /// Created bundle identifiers.
    pub bundles: Vec<String>,
    /// Aggregate row count by date prefix of `start_date`/row-time.
    pub aggregate_by_day: BTreeMap<String, u64>,
}

/// Synthetic journal fixture write failures.
#[derive(Debug)]
pub enum BodySeedError {
    /// Filesystem failure.
    Io { path: PathBuf, source: io::Error },
    /// Manifest or row serialization failure.
    Json(serde_json::Error),
    /// SQLite aggregate write failure.
    Sqlite(rusqlite::Error),
    /// A directly seeded aggregate row lacks its required fields.
    InvalidAggregateRow { bundle: String, shard: String },
}

impl fmt::Display for BodySeedError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Io { path, source } => {
                write!(formatter, "could not write {}: {source}", path.display())
            }
            Self::Json(source) => write!(
                formatter,
                "could not serialize synthetic body data: {source}"
            ),
            Self::Sqlite(source) => {
                write!(formatter, "could not seed synthetic aggregate: {source}")
            }
            Self::InvalidAggregateRow { bundle, shard } => {
                write!(
                    formatter,
                    "synthetic aggregate row is incomplete in {bundle}/{shard}"
                )
            }
        }
    }
}

impl std::error::Error for BodySeedError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            Self::Json(source) => Some(source),
            Self::Sqlite(source) => Some(source),
            Self::InvalidAggregateRow { .. } => None,
        }
    }
}

/// Writes an explicit synthetic journal fixture and optionally its aggregate.
pub fn seed_body_journal(
    journal_root: impl AsRef<Path>,
    seed: &BodyJournalSeed,
) -> Result<BodySeedReport, BodySeedError> {
    let root = journal_root.as_ref();
    for date in &seed.dates {
        create_dir(root.join("chronicle").join(date))?;
    }
    let imports = root.join("imports");
    create_dir(imports.clone())?;
    for bundle in &seed.bundles {
        write_bundle(&imports, bundle)?;
    }
    let aggregate_by_day = match seed.aggregate {
        BodyAggregateSeed::Absent => BTreeMap::new(),
        BodyAggregateSeed::Direct => write_aggregate(root, &seed.bundles)?,
    };
    Ok(BodySeedReport {
        dates: seed.dates.clone(),
        bundles: seed
            .bundles
            .iter()
            .map(|bundle| bundle.import_id.clone())
            .collect(),
        aggregate_by_day,
    })
}

fn write_bundle(imports: &Path, bundle: &BodySeedBundle) -> Result<(), BodySeedError> {
    let bundle_path = imports.join(&bundle.import_id);
    let normalized = bundle_path.join("normalized");
    create_dir(normalized.clone())?;
    if let BodySeedManifest::Present {
        source_type,
        entry_count,
        extra,
    } = &bundle.manifest
    {
        let mut manifest = extra.clone();
        if let Some(source_type) = source_type {
            manifest.insert("source_type".to_owned(), Value::String(source_type.clone()));
        }
        if let Some(entry_count) = entry_count {
            manifest.insert("entry_count".to_owned(), Value::from(*entry_count));
        }
        write_json(bundle_path.join("manifest.json"), &Value::Object(manifest))?;
    }
    for (month, rows) in &bundle.shards {
        let rows = rows
            .iter()
            .cloned()
            .map(|mut row| {
                row.entry("import_id".to_owned())
                    .or_insert_with(|| Value::String(bundle.import_id.clone()));
                row.entry("source_family".to_owned())
                    .or_insert_with(|| Value::String(bundle.source_family.clone()));
                serde_json::to_string(&Value::Object(row))
            })
            .collect::<Result<Vec<_>, _>>()
            .map_err(BodySeedError::Json)?;
        write_text(
            normalized.join(format!("{month}.jsonl")),
            &format!("{}\n", rows.join("\n")),
        )?;
    }
    Ok(())
}

fn write_aggregate(
    root: &Path,
    bundles: &[BodySeedBundle],
) -> Result<BTreeMap<String, u64>, BodySeedError> {
    let path = root.join("imports/health-dedupe.sqlite");
    let connection = Connection::open(&path).map_err(BodySeedError::Sqlite)?;
    connection
        .execute_batch(SCHEMA)
        .map_err(BodySeedError::Sqlite)?;
    for bundle in bundles {
        for (shard, rows) in &bundle.shards {
            for row in rows {
                let Some(dedupe_key) = string(row, "dedupe_key") else {
                    return Err(BodySeedError::InvalidAggregateRow {
                        bundle: bundle.import_id.clone(),
                        shard: shard.clone(),
                    });
                };
                let Some(record_type) = string(row, "record_type") else {
                    return Err(BodySeedError::InvalidAggregateRow {
                        bundle: bundle.import_id.clone(),
                        shard: shard.clone(),
                    });
                };
                let Some(start_time) = row_time(row) else {
                    return Err(BodySeedError::InvalidAggregateRow {
                        bundle: bundle.import_id.clone(),
                        shard: shard.clone(),
                    });
                };
                let source_family = string(row, "source_family").unwrap_or(&bundle.source_family);
                let value_hash = row.get("value").map(|value| {
                    let encoded = serde_json::to_vec(value).expect("JSON value serializes");
                    format!("{:x}", Sha256::digest(encoded))
                });
                connection
                    .execute(
                        "INSERT OR REPLACE INTO health_dedupe (dedupe_key, source_family, source_record_id, record_type, start_time, end_time, value_hash, first_import_id, last_seen_import_id, normalized_ref, raw_ref, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?8, ?9, ?10, ?5, ?5)",
                        (
                            dedupe_key,
                            source_family,
                            string(row, "source_record_id"),
                            record_type,
                            start_time,
                            string(row, "end_date"),
                            value_hash,
                            string(row, "import_id").unwrap_or(&bundle.import_id),
                            string(row, "normalized_ref"),
                            string(row, "raw_ref"),
                        ),
                    )
                    .map_err(BodySeedError::Sqlite)?;
            }
        }
    }
    drop(connection);
    Ok(read_health_dedupe_stats(root)
        .map_err(|source| BodySeedError::Io {
            path,
            source: io::Error::other(source),
        })?
        .map(|stats| stats.by_day.clone())
        .unwrap_or_default())
}

fn string<'a>(row: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn row_time(row: &Map<String, Value>) -> Option<&str> {
    ["start_date", "start_time", "end_date"]
        .into_iter()
        .find_map(|key| string(row, key))
}

fn create_dir(path: PathBuf) -> Result<(), BodySeedError> {
    fs::create_dir_all(&path).map_err(|source| BodySeedError::Io { path, source })
}

fn write_text(path: PathBuf, contents: &str) -> Result<(), BodySeedError> {
    fs::write(&path, contents).map_err(|source| BodySeedError::Io { path, source })
}

fn write_json(path: PathBuf, value: &Value) -> Result<(), BodySeedError> {
    let contents = serde_json::to_string(value).map_err(BodySeedError::Json)?;
    write_text(path, &contents)
}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::time::{SystemTime, UNIX_EPOCH};

    use rusqlite::Connection;
    use serde_json::{Map, json};

    use super::*;
    use crate::read_health_dedupe_stats;

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-seed-{}",
                SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_nanos()
            ));
            fs::create_dir_all(&path).unwrap();
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

    fn valid_row(key: &str, start_date: &str) -> Map<String, Value> {
        json!({
            "schema":"solstone.health.apple_health.v1",
            "source_family":"apple_health",
            "record_type":"synthetic_heart_rate",
            "dedupe_key":key,
            "start_date":start_date,
            "day":start_date[..10],
            "value":42
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn seed(dates: &[&str], aggregate: BodyAggregateSeed) -> BodyJournalSeed {
        BodyJournalSeed {
            dates: dates
                .iter()
                .map(|date| (*date).to_owned())
                .collect::<BTreeSet<_>>(),
            bundles: vec![BodySeedBundle {
                import_id: "synthetic-apple-a".to_owned(),
                source_family: "apple_health".to_owned(),
                manifest: BodySeedManifest::Present {
                    source_type: Some("apple_health".to_owned()),
                    entry_count: Some(2),
                    extra: Map::new(),
                },
                shards: BTreeMap::from([(
                    "2024-01".to_owned(),
                    vec![
                        valid_row("synthetic-a", "2024-01-03T01:00:00Z"),
                        valid_row("synthetic-b", "2024-01-05T01:00:00Z"),
                    ],
                )]),
            }],
            aggregate,
        }
    }

    #[test]
    fn seeded_journal_aggregate_matches_its_report() {
        let temporary = TempDir::new();
        let report = seed_body_journal(
            temporary.path(),
            &seed(&["20240103", "20240105"], BodyAggregateSeed::Direct),
        )
        .unwrap();
        let stats = read_health_dedupe_stats(temporary.path()).unwrap().unwrap();
        assert_eq!(report.aggregate_by_day, stats.by_day);
        assert_eq!(report.dates.len(), 2);
    }

    #[test]
    fn direct_aggregate_schema_matches_real_rebuild_schema() {
        let direct = TempDir::new();
        seed_body_journal(
            direct.path(),
            &seed(&["20240103"], BodyAggregateSeed::Direct),
        )
        .unwrap();
        let rebuilt = TempDir::new();
        let mut rebuild_seed = seed(&["20240103"], BodyAggregateSeed::Absent);
        rebuild_seed.bundles[0].manifest = BodySeedManifest::Absent;
        seed_body_journal(rebuilt.path(), &rebuild_seed).unwrap();
        solstone_core_body_rebuild::rebuild_body_store(rebuilt.path()).unwrap();
        let schema = |path: &Path| {
            let connection = Connection::open(path.join("imports/health-dedupe.sqlite")).unwrap();
            let mut statement = connection
                .prepare("SELECT type, name, sql FROM sqlite_master WHERE type IN ('table', 'index') ORDER BY type, name")
                .unwrap();
            statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<String>>(2)?,
                    ))
                })
                .unwrap()
                .map(Result::unwrap)
                .map(|(kind, name, sql)| {
                    (
                        kind,
                        name,
                        sql.unwrap_or_default()
                            .split_whitespace()
                            .collect::<String>(),
                    )
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(schema(direct.path()), schema(rebuilt.path()));
    }

    #[test]
    fn distinct_explicit_date_and_bundle_shapes_report_their_own_state() {
        let first = TempDir::new();
        let second = TempDir::new();
        let first_report = seed_body_journal(
            first.path(),
            &seed(&["20240103"], BodyAggregateSeed::Direct),
        )
        .unwrap();
        let mut second_seed = seed(
            &["20231230", "20240103", "20240215"],
            BodyAggregateSeed::Direct,
        );
        second_seed.bundles[0].import_id = "synthetic-apple-b".to_owned();
        second_seed.bundles[0].shards.insert(
            "2024-02".to_owned(),
            vec![valid_row("synthetic-c", "2024-02-15T01:00:00Z")],
        );
        let second_report = seed_body_journal(second.path(), &second_seed).unwrap();
        assert_ne!(first_report.dates, second_report.dates);
        assert_ne!(first_report.bundles, second_report.bundles);
        assert_eq!(second_report.aggregate_by_day.len(), 3);
    }
}
