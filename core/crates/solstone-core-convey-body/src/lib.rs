// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Read-only body-store inventory, shard, aggregate, and health primitives.

mod aggregate;
mod health;
mod inventory;
mod month;
mod router;
mod seed;
mod shard;

pub use aggregate::{
    HealthDedupeStats, HealthDedupeStatsError, HealthDedupeTimeRange, read_health_dedupe_stats,
};
pub use health::{
    BodyStoreHealthError, BodyStoreHealthReason, BodyStoreHealthVerdict, read_body_store_health,
};
pub use inventory::{
    BodyImportInventory, BodyImportInventoryEntry, BodyImportInventoryError, BodyImportSkip,
    ManifestEntryCount, ManifestReadError, read_body_import_inventory,
};
pub use month::{MonthReader, read_normalized_rows};
pub use router::api_router;
pub use seed::{
    BodyAggregateSeed, BodyJournalSeed, BodySeedBundle, BodySeedError, BodySeedManifest,
    BodySeedReport, seed_body_journal,
};
pub use shard::{NormalizedRow, NormalizedValue, ShardReadError, read_normalized_shard};
pub use solstone_core_body_source::{BodyValue, FieldState, ValueState};

#[cfg(test)]
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
}
