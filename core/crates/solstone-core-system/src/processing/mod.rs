// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The owner's deferred-analysis policy, shared by execution and reporting.

use chrono::Utc;
use solstone_core_journal_config::{owner_zone, read_journal_config};
use std::path::Path;

mod display;
pub use display::{publish_display_snapshot, read_display_snapshot};

pub use solstone_core_journal_config::processing::*;

/// An unreadable policy cannot authorize background analysis.
pub fn processing_allowed(journal: &Path) -> bool {
    if solstone_core_journal_io::whole_journal_restore::whole_journal_restore_blocks(journal) {
        return false;
    }
    match policy_at(journal, Utc::now()) {
        Ok((settings, gate)) => !settings.deferred || gate.open,
        Err(error) => {
            log::warn!("processing is held: {error}");
            false
        }
    }
}

pub fn policy_at(
    journal: &Path,
    now: chrono::DateTime<Utc>,
) -> Result<(ProcessingSettings, GateState), String> {
    let config = read_journal_config(journal)
        .map_err(|e| e.to_string())?
        .config
        .unwrap_or_default();
    let settings = processing_settings(&config)?;
    let reading = if settings.gate.display_powersave.enabled {
        display::observe_display()
    } else {
        DisplayPowersaveReading::UNAVAILABLE
    };
    Ok((
        settings,
        evaluate_drain_gate(
            settings,
            now.with_timezone(&owner_zone(journal)).time(),
            reading,
        ),
    ))
}

pub fn is_deferred(journal: &Path) -> Result<bool, String> {
    let config = read_journal_config(journal)
        .map_err(|e| e.to_string())?
        .config
        .unwrap_or_default();
    Ok(processing_settings(&config)?.deferred)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn processing_policy_uses_owner_zone_including_dst_and_ignores_provider() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        for provider in ["local", "openai", "google"] {
            std::fs::write(root.path().join("config/journal.json"), serde_json::json!({
                "identity":{"timezone":"America/Denver"}, "providers":{"active":{"provider":provider}},
                "processing":{"mode":"deferred","gate":{"time_window":{"start":"01:00","end":"02:00"}}}
            }).to_string()).unwrap();
            // Both occurrences of 01:30 during the fall fold are permitted.
            for (stamp, expected) in [
                ("2026-11-01T06:59:00Z", false),
                ("2026-11-01T07:30:00Z", true),
                ("2026-11-01T08:30:00Z", true),
                ("2026-11-01T09:00:00Z", false),
                ("2026-03-08T08:30:00Z", true),
                ("2026-03-08T09:00:00Z", false),
            ] {
                let (settings, gate) = policy_at(root.path(), stamp.parse().unwrap()).unwrap();
                assert!(settings.deferred);
                assert_eq!(gate.open, expected, "{stamp}, {provider}");
            }
        }
        std::fs::write(root.path().join("config/journal.json"), "{").unwrap();
        assert!(!processing_allowed(root.path()));
        assert!(is_deferred(root.path()).is_err());
    }

    #[test]
    fn pending_restore_holds_processing_and_clearing_it_restores_the_policy() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir(root.path().join("config")).unwrap();
        std::fs::write(
            root.path().join("config/journal.json"),
            serde_json::json!({
                "identity": {"timezone": "UTC"},
                "processing": {"mode": "normal"}
            })
            .to_string(),
        )
        .unwrap();
        let before = processing_allowed(root.path());
        let source =
            solstone_core_journal_io::whole_journal_restore::WholeJournalRestoreSource::Byo {
                repository: "repo".into(),
                backend: "s3".into(),
            };
        let record =
            solstone_core_journal_io::whole_journal_restore::publish_whole_journal_restore(
                root.path(),
                &source,
                "0123456789abcdef0123456789abcdef0123456789abcdef0123456789abcdef",
                "/journal",
            )
            .expect("obligation");
        assert!(!processing_allowed(root.path()));
        solstone_core_journal_io::whole_journal_restore::clear_whole_journal_restore(
            root.path(),
            &record,
        )
        .expect("clear");
        assert_eq!(processing_allowed(root.path()), before);
    }
}
