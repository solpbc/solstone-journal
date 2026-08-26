// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Phased supervisor admission. Hosts that need an interlock between child
//! launch and readiness use these phases instead of the convenience `boot`.

use std::path::{Path, PathBuf};

use super::{
    LifecycleError, OrphanSweepOutcome, SupervisorLease, SupervisorLifecycle, epoch_seconds,
    hostname, self_heartbeat_filename,
};
use super::{admission, state, sweep, sync};

pub struct SupervisorBootAdmission {
    journal: PathBuf,
    heartbeat_filename: String,
    machine_id: String,
    now: f64,
    lease: SupervisorLease,
}

pub struct PreReadySupervisorLifecycle {
    journal: PathBuf,
    heartbeat_filename: String,
    machine_id: String,
    now: f64,
    last_orphan_sweep: OrphanSweepOutcome,
    lease: SupervisorLease,
}

impl SupervisorBootAdmission {
    #[cfg(any(target_os = "linux", target_os = "macos"))]
    pub fn acquire(journal: impl AsRef<Path>) -> Result<Self, LifecycleError> {
        let journal = journal.as_ref().to_path_buf();
        let lock = state::open_supervisor_lock(&journal)?;
        let lease = admission::acquire(lock)?;
        let heartbeat_filename = self_heartbeat_filename();
        let machine_id = sync::machine_id();
        let now = epoch_seconds();
        let result = sync::check(&journal, &heartbeat_filename, &machine_id, None, now)?;
        if result.is_boot_conflict() {
            return Err(LifecycleError::SyncConflict(Box::new(result)));
        }
        Ok(Self {
            journal,
            heartbeat_filename,
            machine_id,
            now,
            lease,
        })
    }

    pub fn activate(self) -> Result<PreReadySupervisorLifecycle, LifecycleError> {
        state::write_supervisor_identity(&self.journal, std::process::id())?;
        let last_orphan_sweep =
            sweep::sweep_orphans(&self.journal, std::time::Duration::from_secs(1));
        Ok(PreReadySupervisorLifecycle {
            journal: self.journal,
            heartbeat_filename: self.heartbeat_filename,
            machine_id: self.machine_id,
            now: self.now,
            last_orphan_sweep,
            lease: self.lease,
        })
    }
}

impl PreReadySupervisorLifecycle {
    pub fn heartbeat_filename(&self) -> &str {
        &self.heartbeat_filename
    }
    pub fn publish_heartbeat(self) -> Result<SupervisorLifecycle, LifecycleError> {
        let heartbeat = sync::Heartbeat {
            schema: 1,
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
        if let Err(error) =
            state::write_sync_heartbeat(&self.journal, &self.heartbeat_filename, &heartbeat_bytes)
        {
            let _ = self.abort_pre_ready();
            return Err(error);
        }
        Ok(SupervisorLifecycle {
            journal: self.journal,
            heartbeat_filename: self.heartbeat_filename,
            last_orphan_sweep: self.last_orphan_sweep,
            _lease: self.lease,
        })
    }

    pub fn abort_pre_ready(self) -> Result<(), LifecycleError> {
        state::clear_ready(&self.journal)?;
        state::clear_supervisor_identity(&self.journal)
    }
}
