// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};

/// The literal fuzzy threshold the retired Python migration compared with `>=`.
///
/// Pinned here rather than taken from any matcher default: this migration's
/// merge decisions are historical, so the number must not move when a generic
/// caller-supplied default does.
const FUZZY_THRESHOLD: u8 = 90;

pub fn migrate_to_journal_entities(c: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_facets::migrate_legacy_facet_entities(c.journal, FUZZY_THRESHOLD, c.dry_run)
    {
        Ok(r) => MaintBodyResult {
            stdout: vec![
                format!("Entities loaded:       {}", r.loaded),
                format!("Canonical entities:    {}", r.canonicals),
                format!("Merges performed:      {}", r.merges),
                format!("Relationships created: {}", r.relationships),
            ],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
