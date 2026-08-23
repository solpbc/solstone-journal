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
use solstone_core_journal_io::{
    NameAdmissionError, StrictCreateError, create_segment_strict, preflight_segment_admission,
};

use crate::{SOURCE_APPLE_HEALTH, health_card_stream};

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
    /// Day-summary transcript content keyed by its `YYYYMMDD` chronicle day.
    pub day_summaries: BTreeMap<String, String>,
    /// Explicit import bundle specifications.
    pub bundles: Vec<BodySeedBundle>,
    /// Whether the health aggregate is directly written.
    pub aggregate: BodyAggregateSeed,
    /// Optional verbatim contents for `config/journal.json`.
    pub journal_config: Option<Map<String, Value>>,
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
    /// Create a present aggregate database with no rows.
    Empty,
}

/// Description of the synthetic journal state written by the seeder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BodySeedReport {
    /// Created journal date directories.
    pub dates: BTreeSet<String>,
    /// Created bundle identifiers.
    pub bundles: Vec<String>,
    /// Seeded row count by the `YYYYMMDD` date prefix of each row's own `start_date`.
    pub rows_by_start_date_day: BTreeMap<String, u64>,
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
    /// A seed day was not a plain `YYYYMMDD` value.
    InvalidDay { value: String },
    /// A bundle import ID was not a plain path component.
    InvalidImportId { value: String },
    /// A normalized shard name was not a plain path component.
    InvalidShardName { value: String },
    /// Direct aggregate seeding refuses to overwrite an existing aggregate.
    AggregateAlreadyExists { path: PathBuf },
    /// A directly seeded aggregate row lacks its required fields.
    InvalidAggregateRow {
        /// Bundle containing the row.
        bundle: String,
        /// Shard containing the row.
        shard: String,
        /// Missing required field.
        field: &'static str,
    },
    /// Chronicle stream/segment admission refused the synthetic day summary.
    JournalPath(NameAdmissionError),
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
            Self::InvalidDay { value } => {
                write!(formatter, "synthetic seed day must be YYYYMMDD: {value}")
            }
            Self::InvalidImportId { value } => {
                write!(
                    formatter,
                    "synthetic import ID must be one plain path component: {value}"
                )
            }
            Self::InvalidShardName { value } => {
                write!(
                    formatter,
                    "synthetic shard name must be one plain path component: {value}"
                )
            }
            Self::AggregateAlreadyExists { path } => {
                write!(
                    formatter,
                    "refusing to overwrite existing aggregate {}",
                    path.display()
                )
            }
            Self::InvalidAggregateRow {
                bundle,
                shard,
                field,
            } => {
                write!(
                    formatter,
                    "synthetic aggregate row is missing {field} in {bundle}/{shard}"
                )
            }
            Self::JournalPath(error) => {
                write!(formatter, "could not place synthetic day summary: {error}")
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
            Self::InvalidDay { .. }
            | Self::InvalidImportId { .. }
            | Self::InvalidShardName { .. }
            | Self::AggregateAlreadyExists { .. }
            | Self::InvalidAggregateRow { .. } => None,
            Self::JournalPath(error) => Some(error),
        }
    }
}

/// Writes an explicit synthetic journal fixture and optionally its aggregate.
pub fn seed_body_journal(
    journal_root: impl AsRef<Path>,
    seed: &BodyJournalSeed,
) -> Result<BodySeedReport, BodySeedError> {
    let root = journal_root.as_ref();
    let rows_by_start_date_day = prevalidate_seed(root, seed)?;
    for date in &seed.dates {
        create_dir(root.join("chronicle").join(date))?;
    }
    let stream = health_card_stream(SOURCE_APPLE_HEALTH)
        .expect("Apple Health has a day-summary card stream");
    for (day, transcript) in &seed.day_summaries {
        let summary = match create_segment_strict(root, day, stream, SYNTHETIC_DAY_SUMMARY_SEGMENT)
        {
            Ok(path) => path,
            Err(StrictCreateError::CreateIo { path, source }) => {
                return Err(BodySeedError::Io { path, source });
            }
            // Preflight already ran; this arm is only reachable if a
            // non-cooperating actor mutated the journal between prevalidate
            // and create (TOCTOU). Do not panic.
            Err(StrictCreateError::Admission(error)) => {
                return Err(BodySeedError::JournalPath(error));
            }
        };
        write_text(summary.join(DAY_SUMMARY_FILE), transcript)?;
    }
    let imports = root.join("imports");
    create_dir(imports.clone())?;
    for bundle in &seed.bundles {
        write_bundle(&imports, bundle)?;
    }
    match seed.aggregate {
        BodyAggregateSeed::Absent => {}
        BodyAggregateSeed::Direct => write_aggregate(root, &seed.bundles)?,
        BodyAggregateSeed::Empty => write_aggregate(root, &[])?,
    }
    if let Some(config) = &seed.journal_config {
        let config_dir = root.join("config");
        create_dir(config_dir.clone())?;
        write_json(
            config_dir.join("journal.json"),
            &Value::Object(config.clone()),
        )?;
    }
    let mut dates = seed.dates.clone();
    dates.extend(seed.day_summaries.keys().cloned());

    Ok(BodySeedReport {
        dates,
        bundles: seed
            .bundles
            .iter()
            .map(|bundle| bundle.import_id.clone())
            .collect(),
        rows_by_start_date_day,
    })
}

fn prevalidate_seed(
    root: &Path,
    seed: &BodyJournalSeed,
) -> Result<BTreeMap<String, u64>, BodySeedError> {
    for date in &seed.dates {
        if !is_day_key(date) {
            return Err(BodySeedError::InvalidDay {
                value: date.clone(),
            });
        }
    }
    let summary_stream = health_card_stream(SOURCE_APPLE_HEALTH)
        .expect("Apple Health has a day-summary card stream");
    for day in seed.day_summaries.keys() {
        if !is_day_key(day) {
            return Err(BodySeedError::InvalidDay { value: day.clone() });
        }
        preflight_segment_admission(root, day, summary_stream, SYNTHETIC_DAY_SUMMARY_SEGMENT)
            .map_err(BodySeedError::JournalPath)?;
    }
    let mut rows_by_start_date_day = BTreeMap::new();
    for bundle in &seed.bundles {
        if !is_plain_component(&bundle.import_id) {
            return Err(BodySeedError::InvalidImportId {
                value: bundle.import_id.clone(),
            });
        }
        for (shard, rows) in &bundle.shards {
            if !is_plain_component(shard) {
                return Err(BodySeedError::InvalidShardName {
                    value: shard.clone(),
                });
            }
            for row in rows {
                if let Some(start_date) = string(row, "start_date") {
                    let day = start_date
                        .chars()
                        .take(10)
                        .filter(|character| *character != '-')
                        .collect::<String>();
                    if !day.is_empty() {
                        *rows_by_start_date_day.entry(day).or_default() += 1;
                    }
                }
                if seed.aggregate == BodyAggregateSeed::Direct {
                    validate_aggregate_row(bundle, shard, row)?;
                }
            }
        }
    }
    if seed.aggregate != BodyAggregateSeed::Absent {
        let aggregate = root.join("imports/health-dedupe.sqlite");
        match fs::symlink_metadata(&aggregate) {
            Ok(_) => return Err(BodySeedError::AggregateAlreadyExists { path: aggregate }),
            Err(source) if source.kind() == io::ErrorKind::NotFound => {}
            Err(source) => {
                return Err(BodySeedError::Io {
                    path: aggregate,
                    source,
                });
            }
        }
    }
    Ok(rows_by_start_date_day)
}

fn validate_aggregate_row(
    bundle: &BodySeedBundle,
    shard: &str,
    row: &Map<String, Value>,
) -> Result<(), BodySeedError> {
    for field in ["dedupe_key", "record_type"] {
        if string(row, field).is_none() {
            return Err(BodySeedError::InvalidAggregateRow {
                bundle: bundle.import_id.clone(),
                shard: shard.to_owned(),
                field,
            });
        }
    }
    if row_time(row).is_none() {
        return Err(BodySeedError::InvalidAggregateRow {
            bundle: bundle.import_id.clone(),
            shard: shard.to_owned(),
            field: "start_date, start_time, or end_date",
        });
    }
    Ok(())
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

fn write_aggregate(root: &Path, bundles: &[BodySeedBundle]) -> Result<(), BodySeedError> {
    let path = root.join("imports/health-dedupe.sqlite");
    let connection = Connection::open(&path).map_err(BodySeedError::Sqlite)?;
    connection
        .execute_batch(SCHEMA)
        .map_err(BodySeedError::Sqlite)?;
    for bundle in bundles {
        for rows in bundle.shards.values() {
            for row in rows {
                let dedupe_key = string(row, "dedupe_key").expect("prevalidated dedupe key");
                let record_type = string(row, "record_type").expect("prevalidated record type");
                let start_time = row_time(row).expect("prevalidated row time");
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
    Ok(())
}

fn string<'a>(row: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    row.get(key).and_then(Value::as_str)
}

fn row_time(row: &Map<String, Value>) -> Option<&str> {
    ["start_date", "start_time", "end_date"]
        .into_iter()
        .find_map(|key| string(row, key))
}

fn is_day_key(value: &str) -> bool {
    value.len() == 8 && value.bytes().all(|byte| byte.is_ascii_digit())
}

fn is_plain_component(value: &str) -> bool {
    !value.is_empty()
        && value != "."
        && value != ".."
        && !Path::new(value).is_absolute()
        && !value.contains('/')
        && !value.contains('\\')
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

const SYNTHETIC_DAY_SUMMARY_SEGMENT: &str = "000000_300";
const DAY_SUMMARY_FILE: &str = "day_summary_transcript.md";

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use serde_json::{Map, json};

    use super::*;
    use crate::read_health_dedupe_stats;

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-seed-{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
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
            day_summaries: BTreeMap::new(),
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
            journal_config: None,
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
        assert_eq!(report.rows_by_start_date_day, stats.by_day);
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
        assert_eq!(second_report.rows_by_start_date_day.len(), 3);
    }

    #[test]
    fn report_counts_start_dates_without_an_aggregate() {
        let temporary = TempDir::new();
        let report = seed_body_journal(
            temporary.path(),
            &seed(&["20240103"], BodyAggregateSeed::Absent),
        )
        .unwrap();
        assert_eq!(
            report.rows_by_start_date_day,
            BTreeMap::from([("20240103".to_owned(), 1), ("20240105".to_owned(), 1)])
        );
        assert!(
            !temporary
                .path()
                .join("imports/health-dedupe.sqlite")
                .exists()
        );
    }

    #[test]
    fn empty_aggregate_is_present_without_seeded_rows() {
        let temporary = TempDir::new();
        seed_body_journal(
            temporary.path(),
            &seed(&["20240103"], BodyAggregateSeed::Empty),
        )
        .unwrap();
        let stats = read_health_dedupe_stats(temporary.path()).unwrap().unwrap();
        assert_eq!(stats.total, 0);
    }

    #[test]
    fn journal_config_is_written_only_when_seeded() {
        let absent = TempDir::new();
        seed_body_journal(
            absent.path(),
            &seed(&["20240103"], BodyAggregateSeed::Absent),
        )
        .unwrap();
        assert!(!absent.path().join("config/journal.json").exists());

        let present = TempDir::new();
        let mut fixture = seed(&["20240103"], BodyAggregateSeed::Absent);
        fixture.journal_config = Some(Map::from_iter([(
            "body".to_owned(),
            json!({"freshness": {}}),
        )]));
        seed_body_journal(present.path(), &fixture).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(
                &fs::read_to_string(present.path().join("config/journal.json")).unwrap()
            )
            .unwrap(),
            json!({"body": {"freshness": {}}})
        );
    }

    #[test]
    fn rejects_unsafe_paths_before_writing_the_journal() {
        let temporary = TempDir::new();
        let mut invalid = seed(&["20240103"], BodyAggregateSeed::Direct);
        invalid.dates = BTreeSet::from(["../outside".to_owned()]);
        assert!(matches!(
            seed_body_journal(temporary.path(), &invalid),
            Err(BodySeedError::InvalidDay { value }) if value == "../outside"
        ));
        assert!(!temporary.path().join("imports").exists());

        let mut invalid = seed(&["20240103"], BodyAggregateSeed::Direct);
        invalid.bundles[0].import_id = "../outside".to_owned();
        assert!(matches!(
            seed_body_journal(temporary.path(), &invalid),
            Err(BodySeedError::InvalidImportId { value }) if value == "../outside"
        ));
        assert!(!temporary.path().join("imports").exists());

        let mut invalid = seed(&["20240103"], BodyAggregateSeed::Direct);
        let rows = invalid.bundles[0].shards.remove("2024-01").unwrap();
        invalid.bundles[0]
            .shards
            .insert("../outside".to_owned(), rows);
        assert!(matches!(
            seed_body_journal(temporary.path(), &invalid),
            Err(BodySeedError::InvalidShardName { value }) if value == "../outside"
        ));
        assert!(!temporary.path().join("imports").exists());
    }

    #[test]
    fn invalid_direct_rows_and_existing_aggregate_fail_before_writes() {
        let temporary = TempDir::new();
        let mut invalid = seed(&["20240103"], BodyAggregateSeed::Direct);
        invalid.bundles[0].shards.get_mut("2024-01").unwrap()[0].remove("dedupe_key");
        assert!(matches!(
            seed_body_journal(temporary.path(), &invalid),
            Err(BodySeedError::InvalidAggregateRow {
                field: "dedupe_key",
                ..
            })
        ));
        assert!(!temporary.path().join("chronicle").exists());
        assert!(!temporary.path().join("imports").exists());

        let imports = temporary.path().join("imports");
        fs::create_dir_all(&imports).unwrap();
        fs::write(
            imports.join("health-dedupe.sqlite"),
            "real-journal-sentinel",
        )
        .unwrap();
        let valid = seed(&["20240103"], BodyAggregateSeed::Direct);
        assert!(matches!(
            seed_body_journal(temporary.path(), &valid),
            Err(BodySeedError::AggregateAlreadyExists { .. })
        ));
        assert_eq!(
            fs::read_to_string(imports.join("health-dedupe.sqlite")).unwrap(),
            "real-journal-sentinel"
        );
    }

    #[test]
    fn seeded_day_summary_is_readable_through_chronicle_reader() {
        let temporary = TempDir::new();
        let mut fixture = seed(&["20240103"], BodyAggregateSeed::Absent);
        fixture
            .day_summaries
            .insert("20240103".to_owned(), "seeded summary".to_owned());
        seed_body_journal(temporary.path(), &fixture).unwrap();
        assert_eq!(
            crate::find_day_summary(temporary.path(), "20240103").unwrap(),
            Some("seeded summary".to_owned())
        );
    }

    #[test]
    fn prevalidate_rejects_case_variant_stream_before_writes() {
        let temporary = TempDir::new();
        let planted = temporary
            .path()
            .join("chronicle/20240103")
            .join("Import.Apple_Health");
        fs::create_dir_all(&planted).unwrap();
        let mut fixture = seed(&["20240103"], BodyAggregateSeed::Direct);
        fixture
            .day_summaries
            .insert("20240103".to_owned(), "seeded summary".to_owned());
        match seed_body_journal(temporary.path(), &fixture) {
            Err(BodySeedError::JournalPath(_)) => {}
            other => panic!("{other:?}"),
        }
        assert!(!temporary.path().join("imports").exists());
        assert!(
            !temporary
                .path()
                .join("chronicle/20240103/import.apple_health")
                .exists()
        );
        assert!(planted.is_dir());
    }
}
