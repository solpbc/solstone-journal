// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Quiet structural repairs that run with daily housekeeping.
//!
//! Each pass repairs derived or structural state deterministically and never
//! needs an owner decision, so nothing here prints to the routine's output.
//! What a pass did, and anything it left alone, goes to the log.

use std::path::Path;

use solstone_core_segment::LockOptions;

/// Run every quiet repair once. A failing pass is logged and the next still runs.
/// `day` (`YYYYMMDD`, the owner's today) dates any new duplicate suggestion.
pub(crate) fn run_self_heal(journal: &Path, day: &str) {
    heal_stream_tails(journal);
    heal_facet_links(journal);
    suggest_duplicates(journal, day);
}

/// Possible duplicate people found by shared email, shared alias or a similar
/// name across facets become open suggestions for the owner to review. This
/// never merges anything.
fn suggest_duplicates(journal: &Path, day: &str) {
    if !journal.join("entities").is_dir() {
        return;
    }
    match solstone_core_talent_runtime::entities::duplicates::record_duplicate_candidates(
        journal, day,
    ) {
        Ok(discovery) if discovery.recorded > 0 => {
            log::info!(
                "possible duplicates: {} new suggestions",
                discovery.recorded
            );
        }
        Ok(_) => {}
        Err(error) => log::warn!("possible duplicate discovery skipped: {error}"),
    }
}

fn heal_stream_tails(journal: &Path) {
    match solstone_core_segment::self_heal_stream_tails(journal, LockOptions::default()) {
        Ok(report) => {
            if !report.unchanged() {
                log::info!(
                    "stream tails: rewound {}, rebuilt {}",
                    report.rewound.len(),
                    report.backfilled.len()
                );
            }
            for (name, hold) in &report.held {
                log::info!("stream tail left as is: {name} ({hold:?})");
            }
        }
        Err(error) => log::warn!("stream tail repair skipped: {error}"),
    }
}

fn heal_facet_links(journal: &Path) {
    match solstone_core_facets::self_heal_facet_links(journal) {
        Ok(report) => {
            for failure in &report.failures {
                log::warn!("facet link repair step failed: {failure}");
            }
        }
        Err(error) => log::warn!("facet link repair skipped: {error}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quiet_repairs_create_no_stream_records_in_an_empty_journal() {
        let root = tempfile::tempdir().expect("journal root");
        // An empty journal: nothing to repair, nothing created.
        run_self_heal(root.path(), "20260301");
        assert!(!root.path().join("streams").exists());
    }
}
