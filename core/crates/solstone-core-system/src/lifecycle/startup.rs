// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Phased supervisor admission. Hosts that need an interlock between child
//! launch and readiness use these phases instead of the convenience `boot`.

use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use solstone_core_journal_io::{
    BoundParentLock, FlatDirectory, JournalRoot, create_or_open_flat_directory_bound,
};

use super::{
    LifecycleError, OrphanSweepOutcome, SupervisorLifecycle, epoch_seconds, hostname,
    self_heartbeat_filename,
};
use super::{state, sweep, sync};

pub struct SupervisorBootAdmission {
    journal: PathBuf,
    root: JournalRoot,
    health: FlatDirectory,
    sync: FlatDirectory,
    lease: BoundParentLock,
    heartbeat_filename: String,
    machine_id: String,
    now: f64,
}

pub struct PreReadySupervisorLifecycle {
    journal: PathBuf,
    root: JournalRoot,
    health: FlatDirectory,
    sync: FlatDirectory,
    lease: BoundParentLock,
    heartbeat_filename: String,
    machine_id: String,
    now: f64,
    last_orphan_sweep: OrphanSweepOutcome,
}

impl SupervisorBootAdmission {
    /// Bind `health`/`sync` beneath a resolved journal root, retain the
    /// singleton lock, and reject a live foreign writer before any identity
    /// or heartbeat write.
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn acquire(journal: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        let journal = journal.as_ref().to_path_buf();
        let root = JournalRoot::open(&journal).map_err(|error| {
            LifecycleError::SyncScan(Box::new(sync::directory_binding_from_root(&journal, error)))
        })?;
        let lock = state::open_supervisor_lock(&root).map_err(|error| match error {
            state::SupervisorLockError::AlreadyRunning => LifecycleError::AlreadyRunning,
            state::SupervisorLockError::BindHealth(reason) => {
                LifecycleError::SyncScan(Box::new(sync::SyncScanFailure::DirectoryBinding {
                    path: root.canonical_path().join("health"),
                    operation: sync::SyncDirectoryOperation::BindHealth,
                    reason: Box::new(reason),
                }))
            }
            state::SupervisorLockError::Acquire(error) => LifecycleError::SupervisorLock(error),
        })?;
        let state::SupervisorLock { health, lease } = lock;
        let health_diagnostic = root.canonical_path().join("health");
        let sync = create_or_open_flat_directory_bound(
            &health,
            OsStr::new("sync"),
            0o700,
            &health_diagnostic,
        )
        .map_err(|reason| {
            LifecycleError::SyncScan(Box::new(sync::SyncScanFailure::DirectoryBinding {
                path: health_diagnostic.join("sync"),
                operation: sync::SyncDirectoryOperation::BindSync,
                reason: Box::new(reason),
            }))
        })?;

        let heartbeat_filename = self_heartbeat_filename();
        let machine_id = sync::machine_id();
        let now = epoch_seconds();
        let result = sync::scan_bound_sync(&sync, &heartbeat_filename, &machine_id, None, now)
            .map_err(|failure| LifecycleError::SyncScan(Box::new(failure)))?;
        if result.is_boot_conflict() {
            return Err(LifecycleError::SyncConflict(Box::new(result)));
        }
        Ok(Self {
            journal,
            root,
            health,
            sync,
            lease,
            heartbeat_filename,
            machine_id,
            now,
        })
    }

    pub fn activate(self) -> Result<PreReadySupervisorLifecycle, LifecycleError> {
        state::write_supervisor_identity(&self.journal, std::process::id())?;
        let last_orphan_sweep =
            sweep::sweep_orphans(&self.journal, std::time::Duration::from_secs(1));
        Ok(PreReadySupervisorLifecycle {
            journal: self.journal,
            root: self.root,
            health: self.health,
            sync: self.sync,
            lease: self.lease,
            heartbeat_filename: self.heartbeat_filename,
            machine_id: self.machine_id,
            now: self.now,
            last_orphan_sweep,
        })
    }
}

impl PreReadySupervisorLifecycle {
    pub fn heartbeat_filename(&self) -> &str {
        &self.heartbeat_filename
    }

    pub fn publish_heartbeat(self) -> Result<SupervisorLifecycle, LifecycleError> {
        let heartbeat = sync::Heartbeat {
            schema: sync::HEARTBEAT_SCHEMA_V1,
            machine_id: self.machine_id.clone(),
            hostname: hostname(),
            pid: std::process::id(),
            wall_time: self.now.to_string(),
            solstone_version: env!("CARGO_PKG_VERSION").to_owned(),
            interval_seconds: sync::DEFAULT_INTERVAL_SECONDS as u32,
            journal_path: self.journal.display().to_string(),
        };
        let heartbeat_bytes = match serde_json::to_vec(&heartbeat) {
            Ok(bytes) => bytes,
            Err(error) => {
                let _ = self.abort_pre_ready();
                return Err(error.into());
            }
        };
        let retained_self_heartbeat = match state::write_sync_heartbeat(
            &self.sync,
            &self.heartbeat_filename,
            &heartbeat_bytes,
        ) {
            Ok(observation) => observation,
            Err(error) => {
                let _ = self.abort_pre_ready();
                return Err(error.into());
            }
        };
        Ok(SupervisorLifecycle {
            journal: self.journal,
            heartbeat_filename: self.heartbeat_filename,
            last_orphan_sweep: self.last_orphan_sweep,
            _journal_root: self.root,
            _health: self.health,
            sync: self.sync,
            _lease: self.lease,
            retained_self_heartbeat: Some(retained_self_heartbeat),
            last_completed_sync_result: None,
        })
    }

    pub fn abort_pre_ready(self) -> Result<(), LifecycleError> {
        state::clear_ready(&self.journal)?;
        state::clear_supervisor_identity(&self.journal)
    }
}
