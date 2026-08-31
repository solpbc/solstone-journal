// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::fmt;
use std::path::Path;

use solstone_core_journal_io::lock_is_held;

use crate::{
    BodyImportInventoryError, HealthDedupeStatsError, ManifestEntryCount,
    read_body_import_inventory, read_health_dedupe_stats,
};

/// Read-only body-store health verdict.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BodyStoreHealthVerdict {
    /// No visible bundle claims rows and no aggregate rows are available.
    FirstRun(BodyStoreHealthReason),
    /// The aggregate contains rows.
    Healthy(BodyStoreHealthReason),
    /// A positive-row claim disagrees with the aggregate while rebuild owns its lock.
    Rebuilding(BodyStoreHealthReason),
    /// A positive-row claim disagrees with an absent, unreadable, or empty aggregate.
    Torn(BodyStoreHealthReason),
}

/// Stable machine-readable explanation for a body-store health verdict.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BodyStoreHealthReason {
    /// Nothing claims rows and no rows can be read from the aggregate.
    NoClaimedRowsAndNoAggregateRows,
    /// The aggregate has at least one row.
    AggregateHasRows,
    /// The rebuild lock sidecar is exclusively held.
    RebuildLockHeld,
    /// Claimed rows exist but the aggregate database file does not.
    AggregateMissing,
    /// Claimed rows exist but the aggregate cannot be queried.
    AggregateUnreadable,
    /// Claimed rows exist but the aggregate has no rows.
    AggregateEmpty,
}

impl BodyStoreHealthReason {
    /// Returns the stable route-serving reason string.
    pub const fn as_str(&self) -> &'static str {
        match self {
            Self::NoClaimedRowsAndNoAggregateRows => "no_claimed_rows_and_no_aggregate_rows",
            Self::AggregateHasRows => "aggregate_has_rows",
            Self::RebuildLockHeld => "rebuild_lock_held",
            Self::AggregateMissing => "aggregate_missing",
            Self::AggregateUnreadable => "aggregate_unreadable",
            Self::AggregateEmpty => "aggregate_empty",
        }
    }
}

/// Health-read failures that cannot become a verdict.
#[derive(Debug)]
pub enum BodyStoreHealthError {
    /// Inventory directory failure.
    Inventory(BodyImportInventoryError),
    /// Lock probe failure for a torn candidate.
    Lock(solstone_core_journal_io::LockError),
    /// An aggregate without positive manifest claims was unreadable.
    Aggregate(HealthDedupeStatsError),
}

impl fmt::Display for BodyStoreHealthError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Inventory(source) => write!(formatter, "could not read body inventory: {source}"),
            Self::Lock(source) => write!(formatter, "could not probe body rebuild lock: {source}"),
            Self::Aggregate(source) => write!(formatter, "could not read body aggregate: {source}"),
        }
    }
}

impl std::error::Error for BodyStoreHealthError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Inventory(source) => Some(source),
            Self::Lock(source) => Some(source),
            Self::Aggregate(source) => Some(source),
        }
    }
}

/// Reads inventory first, then aggregate state, probing the rebuild lock only for torn candidates.
pub fn read_body_store_health(
    journal_root: impl AsRef<Path>,
) -> Result<BodyStoreHealthVerdict, BodyStoreHealthError> {
    let root = journal_root.as_ref();
    let inventory = read_body_import_inventory(root).map_err(BodyStoreHealthError::Inventory)?;
    let claims_rows = inventory
        .entries
        .iter()
        .any(|entry| matches!(entry.entry_count, ManifestEntryCount::Present(count) if count > 0));
    match read_health_dedupe_stats(root) {
        Ok(Some(stats)) if stats.total > 0 => Ok(BodyStoreHealthVerdict::Healthy(
            BodyStoreHealthReason::AggregateHasRows,
        )),
        Ok(None) if !claims_rows => Ok(BodyStoreHealthVerdict::FirstRun(
            BodyStoreHealthReason::NoClaimedRowsAndNoAggregateRows,
        )),
        Ok(Some(_)) if !claims_rows => Ok(BodyStoreHealthVerdict::FirstRun(
            BodyStoreHealthReason::NoClaimedRowsAndNoAggregateRows,
        )),
        Ok(None) => torn_or_rebuilding(root, BodyStoreHealthReason::AggregateMissing),
        Ok(Some(_)) => torn_or_rebuilding(root, BodyStoreHealthReason::AggregateEmpty),
        Err(error) if !claims_rows => Err(BodyStoreHealthError::Aggregate(error)),
        Err(_) => torn_or_rebuilding(root, BodyStoreHealthReason::AggregateUnreadable),
    }
}

fn torn_or_rebuilding(
    root: &Path,
    torn_reason: BodyStoreHealthReason,
) -> Result<BodyStoreHealthVerdict, BodyStoreHealthError> {
    let aggregate = root.join("imports/health-dedupe.sqlite");
    if lock_is_held(aggregate).map_err(BodyStoreHealthError::Lock)? {
        return Ok(BodyStoreHealthVerdict::Rebuilding(
            BodyStoreHealthReason::RebuildLockHeld,
        ));
    }
    Ok(BodyStoreHealthVerdict::Torn(torn_reason))
}

#[cfg(all(test, feature = "full-tests"))]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};
    use std::fs;
    use std::path::{Path, PathBuf};
    use std::sync::atomic::{AtomicU64, Ordering};

    use rusqlite::Connection;
    use serde_json::{Map, Value, json};
    use solstone_core_journal_io::{LockOptions, hold_lock};

    use super::*;
    use crate::{
        BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedManifest, seed_body_journal,
    };

    static SEQUENCE: AtomicU64 = AtomicU64::new(0);

    struct TempDir(PathBuf);

    impl TempDir {
        fn new() -> Self {
            let path = std::env::temp_dir().join(format!(
                "solstone-convey-body-health-{}-{}",
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

    fn row() -> Map<String, Value> {
        json!({
            "schema":"solstone.health.apple_health.v1",
            "source_family":"apple_health",
            "record_type":"synthetic_health_type",
            "dedupe_key":"synthetic-health-key",
            "start_date":"2024-01-05T01:00:00Z",
            "day":"20240105",
            "value":42
        })
        .as_object()
        .unwrap()
        .clone()
    }

    fn journal(entry_count: Option<u64>, aggregate: BodyAggregateSeed) -> BodyJournalSeed {
        BodyJournalSeed {
            dates: BTreeSet::from(["20240105".to_owned()]),
            day_summaries: BTreeMap::new(),
            bundles: vec![BodySeedBundle {
                import_id: "synthetic-health-bundle".to_owned(),
                source_family: "apple_health".to_owned(),
                manifest: BodySeedManifest::Present {
                    source_type: Some("apple_health".to_owned()),
                    entry_count,
                    extra: Map::new(),
                },
                shards: BTreeMap::from([("2024-01".to_owned(), vec![row()])]),
            }],
            aggregate,
            journal_config: None,
        }
    }

    #[test]
    fn positive_claim_with_missing_aggregate_is_torn() {
        let temporary = TempDir::new();
        seed_body_journal(
            temporary.path(),
            &journal(Some(1), BodyAggregateSeed::Absent),
        )
        .unwrap();
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Torn(BodyStoreHealthReason::AggregateMissing)
        );
    }

    #[test]
    fn positive_claim_with_unreadable_aggregate_is_torn() {
        let temporary = TempDir::new();
        seed_body_journal(
            temporary.path(),
            &journal(Some(1), BodyAggregateSeed::Absent),
        )
        .unwrap();
        Connection::open(temporary.path().join("imports/health-dedupe.sqlite"))
            .unwrap()
            .execute_batch("CREATE TABLE wrong_table (id INTEGER)")
            .unwrap();
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Torn(BodyStoreHealthReason::AggregateUnreadable)
        );
    }

    #[test]
    fn no_claims_or_zero_claims_are_not_torn_without_aggregate_rows() {
        let no_bundles = TempDir::new();
        assert_eq!(
            read_body_store_health(no_bundles.path()).unwrap(),
            BodyStoreHealthVerdict::FirstRun(
                BodyStoreHealthReason::NoClaimedRowsAndNoAggregateRows
            )
        );
        let zero_absent = TempDir::new();
        seed_body_journal(
            zero_absent.path(),
            &journal(Some(0), BodyAggregateSeed::Absent),
        )
        .unwrap();
        assert!(matches!(
            read_body_store_health(zero_absent.path()),
            Ok(BodyStoreHealthVerdict::FirstRun(_))
        ));
        let zero_empty = TempDir::new();
        let mut seed = journal(Some(0), BodyAggregateSeed::Direct);
        seed.bundles[0].shards.clear();
        seed_body_journal(zero_empty.path(), &seed).unwrap();
        assert!(matches!(
            read_body_store_health(zero_empty.path()),
            Ok(BodyStoreHealthVerdict::FirstRun(_))
        ));
    }

    #[test]
    fn manifestless_native_rows_rebuilt_for_real_are_healthy_and_invisible_to_inventory() {
        let temporary = TempDir::new();
        let mut seed = journal(Some(1), BodyAggregateSeed::Absent);
        seed.bundles[0].manifest = BodySeedManifest::Absent;
        seed_body_journal(temporary.path(), &seed).unwrap();
        solstone_core_body_rebuild::rebuild_body_store(temporary.path()).unwrap();
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Healthy(BodyStoreHealthReason::AggregateHasRows)
        );
    }

    #[test]
    fn absent_row_count_claims_zero_and_is_not_torn() {
        let temporary = TempDir::new();
        seed_body_journal(temporary.path(), &journal(None, BodyAggregateSeed::Absent)).unwrap();
        assert!(matches!(
            read_body_store_health(temporary.path()),
            Ok(BodyStoreHealthVerdict::FirstRun(_))
        ));
    }

    #[test]
    fn warm_cache_then_delete_becomes_torn_and_completed_sidecar_does_not_rebuild() {
        let temporary = TempDir::new();
        seed_body_journal(
            temporary.path(),
            &journal(Some(1), BodyAggregateSeed::Direct),
        )
        .unwrap();
        assert!(matches!(
            read_body_store_health(temporary.path()),
            Ok(BodyStoreHealthVerdict::Healthy(_))
        ));
        let database = temporary.path().join("imports/health-dedupe.sqlite");
        fs::remove_file(&database).unwrap();
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Torn(BodyStoreHealthReason::AggregateMissing)
        );
        let completed_lock = hold_lock(&database, LockOptions::default()).unwrap();
        drop(completed_lock);
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Torn(BodyStoreHealthReason::AggregateMissing)
        );
    }

    #[test]
    fn held_real_rebuild_lock_reports_rebuilding_and_recovery_is_not_latched() {
        let temporary = TempDir::new();
        seed_body_journal(
            temporary.path(),
            &journal(Some(1), BodyAggregateSeed::Absent),
        )
        .unwrap();
        let database = temporary.path().join("imports/health-dedupe.sqlite");
        let lock = hold_lock(&database, LockOptions::default()).unwrap();
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Rebuilding(BodyStoreHealthReason::RebuildLockHeld)
        );
        drop(lock);
        seed_body_journal(
            temporary.path(),
            &journal(Some(1), BodyAggregateSeed::Direct),
        )
        .unwrap();
        assert_eq!(
            read_body_store_health(temporary.path()).unwrap(),
            BodyStoreHealthVerdict::Healthy(BodyStoreHealthReason::AggregateHasRows)
        );
    }
}
