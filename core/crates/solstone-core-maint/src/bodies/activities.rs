// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};
pub fn migrate_activity_icon_to_emoji(context: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_facets::migrate_custom_activity_icons_to_emoji(
        context.journal,
        context.dry_run,
    ) {
        Ok(r) => MaintBodyResult {
            stdout: vec![format!(
                "Migrated {} custom activity record(s) across {} file(s); scanned {} file(s).",
                r.records_changed, r.files_changed, r.files_scanned
            )],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
