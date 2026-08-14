// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc
use crate::registry::{MaintBodyContext, MaintBodyResult};
pub fn migrate_remote_to_observer(context: &MaintBodyContext<'_>) -> MaintBodyResult {
    match solstone_core_observer::migrate_remote_observer_storage(context.journal) {
        Ok(r) => MaintBodyResult {
            stdout: vec![format!(
                "Migrated {} remote observer file(s).",
                r.moved_files
            )],
            exit_code: 0,
        },
        Err(e) => MaintBodyResult {
            stdout: vec![e.to_string()],
            exit_code: 1,
        },
    }
}
