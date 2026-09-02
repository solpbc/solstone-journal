// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only body-store inventory, shard, aggregate, and health primitives.

mod aggregate;
mod archive;
mod chronicle;
#[cfg(all(test, feature = "full-tests"))]
mod corpus_test;
mod day;
mod freshness;
mod health;
mod inventory;
mod month;
mod presentation;
mod query;
mod router;
mod seed;
mod shard;
mod signature;
mod sleep;
mod trends;
mod window;

pub use aggregate::{
    HealthDedupeStats, HealthDedupeStatsError, HealthDedupeTimeRange, read_health_dedupe_stats,
};
pub use chronicle::{ChronicleReadError, find_day_summary, has_chronicle_day};
pub use health::{
    BodyStoreHealthError, BodyStoreHealthReason, BodyStoreHealthVerdict, read_body_store_health,
};
pub use inventory::{
    BodyImportInventory, BodyImportInventoryEntry, BodyImportInventoryError, BodyImportSkip,
    ManifestEntryCount, ManifestReadError, read_body_import_inventory,
};
pub use month::{MonthReader, coverage_month_keys, read_normalized_rows};
pub use presentation::{
    FRIENDLY_CONTRIBUTOR_NAMES, FRIENDLY_TYPE_NAMES, HEALTH_CARD_STREAM_BY_FAMILY,
    HealthCardStreamError, SOURCE_APPLE_HEALTH, SOURCE_DEXCOM_CLARITY, SOURCE_OURA,
    SOURCE_OURA_API, display_number, display_value, friendly_contributor_name, friendly_type_name,
    friendly_unit_label, health_card_stream,
};
pub use router::api_router;
pub use seed::{
    BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedError, BodySeedManifest,
    BodySeedReport, seed_body_journal,
};
pub use shard::{NormalizedRow, NormalizedValue, ShardReadError, read_normalized_shard};
pub use signature::{DatabaseSignatureError, TrendsSignature, trends_db_path, trends_signature};
pub(crate) use signature::{health_dedupe_database_path, read_database_signature};
pub use sleep::{
    DaySleep, SLEEP_SESSION_GAP_MINUTES, SleepInterval, SleepStagedInterval, merge_sleep_sessions,
    pick_day_sleep, pick_main_session, sleep_stage_kind,
};
pub use solstone_core_body_source::{BodyValue, FieldState, ValueState};
pub use trends::{
    TrendAnnotation, TrendCoverage, TrendSignal, TrendValue, TrendsCacheError, TrendsFoldError,
    TrendsPayload, TrendsWarmOutcome, TrendsWarmProbe, read_trends_cache, replace_trends_cache,
    typical_by_signal, warm_trends,
};

#[cfg(all(test, not(feature = "full-tests")))]
mod tests {
    use std::fs;
    use std::path::{Path, PathBuf};

    #[test]
    fn only_convey_shell_depends_on_this_library_and_ios_excludes_it() {
        let core = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let manifest = fs::read_to_string(core.join("Cargo.toml")).unwrap();
        let members = manifest
            .lines()
            .skip_while(|line| line.trim() != "members = [")
            .skip(1)
            .take_while(|line| line.trim() != "]")
            .filter_map(|line| line.split('"').nth(1))
            .map(PathBuf::from)
            .collect::<Vec<_>>();

        let shell_member = Path::new("crates/solstone-core-convey-shell");
        for member in members {
            let member_manifest =
                fs::read_to_string(core.join(&member).join("Cargo.toml")).unwrap();
            for table in [
                "[dependencies]",
                "[dev-dependencies]",
                "[build-dependencies]",
            ] {
                let Some(table_start) = member_manifest.find(table) else {
                    continue;
                };
                let table_body = &member_manifest[table_start + table.len()..];
                let table_body = table_body.split("\n[").next().unwrap_or(table_body);
                let names_body = table_body
                    .lines()
                    .any(|line| line.trim_start().starts_with("solstone-core-convey-body"));
                if member == shell_member && table == "[dependencies]" {
                    assert!(names_body, "shell depends on solstone-core-convey-body");
                } else {
                    assert!(
                        !names_body,
                        "{} names solstone-core-convey-body in {table}",
                        member.display()
                    );
                }
            }
        }

        let makefile = fs::read_to_string(core.join("..").join("Makefile")).unwrap();
        let ios_recipe = makefile
            .lines()
            .find(|line| line.contains("--target $(IOS_TARGET)"))
            .unwrap();
        assert!(ios_recipe.contains("--exclude solstone-core-convey-body"));
    }

    #[test]
    fn body_manifest_has_no_ipc_dependencies() {
        let manifest =
            fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml")).unwrap();
        for forbidden in ["callosum", "socket", "ipc"] {
            assert!(
                !manifest.contains(forbidden),
                "Body trends must not add {forbidden} to Cargo.toml"
            );
        }
    }
}
