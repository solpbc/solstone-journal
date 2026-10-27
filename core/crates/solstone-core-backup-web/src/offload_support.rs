// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Current local support, without repository work or a cached activation flag.

use crate::BackupWebDeps;
use solstone_core_journal_io::JournalRoot;
use std::sync::Arc;

pub(crate) fn check(deps: &BackupWebDeps, root: &Arc<JournalRoot>) -> Result<(), &'static str> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        if deps.operations.is_stopping() || deps.verification_submitter.is_none() {
            return Err("backup_unavailable");
        }
        if !deps
            .offload_dispatch_probe
            .as_ref()
            .is_some_and(|probe| probe())
        {
            return Err("backup_unavailable");
        }
        let config = solstone_core_backup::archive_admission::ArchiveConfiguration::capture(root)
            .map_err(|_| "archive_configuration_unavailable")?;
        let tools = solstone_core_backup_runtime::resolve_operational_tools(
            deps.runner.as_ref(),
            root.canonical_path(),
            true,
            &deps.executable,
        )
        .map_err(|_| "archive_tools_unavailable")?;
        solstone_core_backup_runtime::check_archive_creator_support(
            root,
            &config,
            tools.restic.invocation(),
            tools.rclone_path.as_deref(),
        )
        .map_err(|_| "archive_backend_unavailable")
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = (deps, root);
        Ok(())
    }
}
