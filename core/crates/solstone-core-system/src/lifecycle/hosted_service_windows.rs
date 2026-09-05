// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Windows boundary for the Unix hosted-service parent contract.
//!
//! Journal's Windows lifecycle is owned by the Task Scheduler and its Job
//! facade, not by the Unix parent-loss coordinator.  This module keeps the
//! shared service graph portable while refusing any attempt to run it under
//! the Unix hosted-parent marker.  In particular, it must never downgrade a
//! marked service to an unhosted invocation: doing so would let it bind and
//! launch workers without the admission contract it was told to require.

use std::ffi::OsStr;
use std::path::Path;

use thiserror::Error;

use super::{HostedServiceKind, ParentLossReason};
use crate::process::HostedLaunchProvenance;

const HOSTED_PARENT_ENV: &str = "SOL_SUPERVISOR_SPAWNED";

/// There is no Windows value for the Unix hosted-service parent runtime.
///
/// Its public methods preserve the shared service signatures.  Admission can
/// never construct one on Windows, so each method is unreachable in safe
/// Rust.  The Windows Task Scheduler/Job lifecycle owns its distinct
/// supervision contract instead.
pub enum HostedServiceParentRuntime {}

/// Service-owned shutdown facts captured before a Unix parent-loss witness
/// would be published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostedServiceShutdownEvidence {
    pub listener_stopped: bool,
    pub service_runner_stopped: bool,
    pub operational_artifacts_cleaned: bool,
}

impl HostedServiceParentRuntime {
    pub fn child_launch_provenance(&self, _launch_id: String) -> HostedLaunchProvenance {
        match *self {}
    }

    pub async fn await_parent_loss(&self) -> ParentLossReason {
        match *self {}
    }

    pub fn retire_expected_requested(&self) -> bool {
        match *self {}
    }

    pub async fn await_parent_loss_or_retire_expected_request(&self) -> Option<ParentLossReason> {
        match *self {}
    }

    pub fn finish_parent_loss(
        &self,
        _shutdown: HostedServiceShutdownEvidence,
    ) -> Result<(), HostedServiceParentLossError> {
        match *self {}
    }
}

/// The hosted Unix parent contract was requested on a platform that has no
/// implementation for it.
#[derive(Debug, Error)]
pub enum HostedServiceAdmissionFailure {
    #[error(
        "hosted Unix service supervision is unavailable on Windows; use the Windows Task Scheduler lifecycle"
    )]
    UnsupportedPlatform,
}

/// Kept as a portable service-signature error; no Windows hosted runtime can
/// reach this operation.
#[derive(Debug, Error)]
pub enum HostedServiceParentLossError {
    #[error("hosted Unix service supervision is unavailable on Windows")]
    UnsupportedPlatform,
}

/// Kept as a portable service-signature error; Windows does not install the
/// Unix parent watcher.
#[derive(Debug, Error)]
pub enum HostedServiceWatchError {
    #[error("hosted Unix service supervision is unavailable on Windows")]
    UnsupportedPlatform,
}

/// Admit no Unix hosted parent on Windows.
///
/// An absent marker preserves the ordinary unhosted service lifecycle.  The
/// exact marker that means "this service must be parent-admitted" fails
/// closed instead of running without the requested supervision.
pub fn admit_hosted_service_parent(
    _journal: &Path,
    _kind: HostedServiceKind,
) -> Result<Option<HostedServiceParentRuntime>, HostedServiceAdmissionFailure> {
    if hosted_parent_marker_is_set(std::env::var_os(HOSTED_PARENT_ENV).as_deref()) {
        return Err(HostedServiceAdmissionFailure::UnsupportedPlatform);
    }
    Ok(None)
}

fn hosted_parent_marker_is_set(value: Option<&OsStr>) -> bool {
    value == Some(OsStr::new("1"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_explicit_hosted_marker_requests_admission() {
        assert!(hosted_parent_marker_is_set(Some(OsStr::new("1"))));
        assert!(!hosted_parent_marker_is_set(None));
        assert!(!hosted_parent_marker_is_set(Some(OsStr::new("0"))));
        assert!(!hosted_parent_marker_is_set(Some(OsStr::new("true"))));
    }
}
