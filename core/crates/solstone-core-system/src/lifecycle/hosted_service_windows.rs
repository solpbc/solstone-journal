// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Hosted Windows services derive their lifetime from exact launch admission.

use super::{HostedServiceKind, ParentLossReason};
use crate::process::{
    AdmittedWindowsLaunch, HostedLaunchProvenance, InstanceVerdict, ProcessInstanceSource,
    SystemProcessInstanceSource,
};
use std::path::Path;
use std::time::Duration;
use thiserror::Error;

#[derive(Debug)]
pub struct HostedServiceParentRuntime {
    admitted: AdmittedWindowsLaunch,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedServiceShutdownEvidence {
    pub listener_stopped: bool,
    pub service_runner_stopped: bool,
    pub operational_artifacts_cleaned: bool,
}

impl HostedServiceParentRuntime {
    pub fn child_launch_provenance(&self, launch_id: String) -> HostedLaunchProvenance {
        self.admitted.child_launch_provenance(launch_id)
    }

    fn parent_loss(&self) -> Option<ParentLossReason> {
        if self.admitted.stop_requested().is_err() {
            return Some(ParentLossReason::Unverifiable);
        }
        match SystemProcessInstanceSource.observe(&self.admitted.parent()) {
            InstanceVerdict::SameLive { .. } => None,
            InstanceVerdict::NotSameOrExited => Some(ParentLossReason::ExitedOrReused),
            InstanceVerdict::Unverifiable => Some(ParentLossReason::Unverifiable),
        }
    }

    pub async fn await_parent_loss(&self) -> ParentLossReason {
        loop {
            if let Some(reason) = self.parent_loss() {
                return reason;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn retire_expected_requested(&self) -> bool {
        self.admitted.stop_requested().unwrap_or(false)
    }

    pub async fn await_parent_loss_or_retire_expected_request(&self) -> Option<ParentLossReason> {
        loop {
            match self.admitted.stop_requested() {
                Ok(true) => return None,
                Err(_) => return Some(ParentLossReason::Unverifiable),
                Ok(false) => {}
            }
            if let Some(reason) = self.parent_loss() {
                return Some(reason);
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub fn finish_parent_loss(
        &self,
        shutdown: HostedServiceShutdownEvidence,
    ) -> Result<(), HostedServiceParentLossError> {
        if shutdown.listener_stopped
            && shutdown.service_runner_stopped
            && shutdown.operational_artifacts_cleaned
        {
            Ok(())
        } else {
            Err(HostedServiceParentLossError::Failed(
                "hosted service cleanup is incomplete".into(),
            ))
        }
    }
}

#[derive(Debug, Error)]
pub enum HostedServiceAdmissionFailure {
    #[error("hosted launch journal or service does not match this command")]
    ProvenanceMismatch,
}

#[derive(Debug, Error)]
pub enum HostedServiceParentLossError {
    #[error("parent loss error: {0}")]
    Failed(String),
}

#[derive(Debug, Error)]
pub enum HostedServiceWatchError {
    #[error("parent watch error: {0}")]
    Failed(String),
}

pub fn admit_hosted_service_parent(
    journal: &Path,
    kind: HostedServiceKind,
    admitted: Option<&AdmittedWindowsLaunch>,
) -> Result<Option<HostedServiceParentRuntime>, HostedServiceAdmissionFailure> {
    let Some(admitted) = admitted else {
        return Ok(None);
    };
    if admitted.journal() != journal || admitted.service() != Some(kind) {
        return Err(HostedServiceAdmissionFailure::ProvenanceMismatch);
    }
    Ok(Some(HostedServiceParentRuntime {
        admitted: admitted.clone(),
    }))
}
