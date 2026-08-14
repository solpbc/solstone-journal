// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};

pub fn migrate_index_stream(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    use solstone_core_indexer_store::migrations::index_stream::{
        IndexStreamMigration, migrate_index_stream as migrate,
    };

    match migrate(c.journal, c.dry_run) {
        Ok(IndexStreamMigration::Absent) => success("No existing index found, nothing to migrate"),
        Ok(IndexStreamMigration::Current) => {
            success("Index schema is current, no migration needed")
        }
        Ok(IndexStreamMigration::Rebuilt { missing }) => MaintBodyResult {
            stdout: vec![
                format!("Index schema outdated (missing: {})", missing.join(", ")),
                "Rebuilding the index with the native writer...".into(),
                "Native index rebuild complete".into(),
            ],
            exit_code: 0,
        },
        Ok(IndexStreamMigration::WouldRebuild { missing }) => success(&format!(
            "[DRY-RUN] Index schema outdated (missing: {})",
            missing.join(", ")
        )),
        Err(error) => MaintBodyResult {
            stdout: vec![error.to_string()],
            exit_code: 1,
        },
    }
}

fn success(line: &str) -> MaintBodyResult {
    MaintBodyResult {
        stdout: vec![line.to_owned()],
        exit_code: 0,
    }
}

pub fn migrate_topic_to_agent(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match (
        solstone_core_facets::migrate_event_topic_keys(c.journal, c.dry_run)
            .map_err(|e| e.to_string()),
        solstone_core_journal_stats_cli::migrate_stats_topic_keys(c.journal, c.dry_run),
    ) {
        (Ok(events), Ok(stats)) => MaintBodyResult {
            stdout: vec![format!(
                "Migrated {} event record(s) and {} stats file(s).",
                events.records_changed, stats.files_changed
            )],
            exit_code: 0,
        },
        (Err(e), _) | (_, Err(e)) => MaintBodyResult {
            stdout: vec![e],
            exit_code: 1,
        },
    }
}
