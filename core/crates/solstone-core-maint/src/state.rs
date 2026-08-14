// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

use std::path::Path;

pub use solstone_core_system_health::{
    MaintStateIntegrity, MaintTaskState, MaintTaskStatus, maint_state_file,
};

use crate::registry::task_definitions;

/// Read the static maint registry together with durable historical state files.
pub fn read_states(journal: &Path) -> Vec<MaintTaskState> {
    let definitions = task_definitions();
    solstone_core_system_health::read_maint_task_states(journal, &definitions)
}
