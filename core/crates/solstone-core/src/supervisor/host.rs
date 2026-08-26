// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! [check] Lifecycle termination classification: the hosted supervisor route, AppService (including
//! restart signalling), and TaskQueue are supervisor-owned exact-instance routes; they use birth-validated
//! direct-PID signalling and must not use process-group fallback. provider_runtime launch/parakeet and
//! retention-client retain the legacy terminate() route and are out of scope for this lifecycle contract.
//! ParentDeathBackstop is deleted; it has no remaining production route. Any new caller must be classified
//! here before selecting exact or legacy termination.

use std::path::{Path, PathBuf};

use solstone_core_cli::SupervisorOptions;
use solstone_core_installation_identity::{
    Generation, journal_token_from_path, load_installation_binding, root_token_from_path,
};
use solstone_core_system::lifecycle::{
    DeclaredParent, LifecycleError, ParentAdmissionFailure, ParentLossReason, ParentWatch,
    ShutdownDisposition, ShutdownPhase, ShutdownReport, SupervisorBootAdmission,
};
use solstone_core_system::process::SystemProcessInstanceSource;

use super::{runtime, tick};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupervisorSignal {
    SigTerm,
    SigInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShutdownCause {
    Signal(SupervisorSignal),
    SyncConflict,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SiblingBinaryResolutionError {
    CurrentExecutable,
    MissingOrNotExecutable { path: PathBuf },
    InvalidLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallationBindingRefusal {
    LoadFailed,
    JournalTokenMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum LifecycleBootError {
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorBootRefusal {
    SyncConflict,
    ParentLiveness(ParentAdmissionFailure),
    ParentLostBeforeReadiness(ParentLossReason),
    SiblingBinaryResolution(SiblingBinaryResolutionError),
    InstallationBinding(InstallationBindingRefusal),
    Lifecycle(LifecycleBootError),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupervisorHostOutcome {
    OrderlyShutdown {
        cause: ShutdownCause,
    },
    ForcedShutdownAfterGraceTimeout {
        cause: ShutdownCause,
        phase: ShutdownPhase,
    },
    Refused {
        reason: SupervisorBootRefusal,
    },
    ParentLost {
        reason: ParentLossReason,
        shutdown: ShutdownDisposition,
    },
}

struct HostedSupervisorAdmission {
    lifecycle: solstone_core_system::lifecycle::PreReadySupervisorLifecycle,
    _generation: Generation,
    parent_watch: Option<ParentWatch>,
}

/// Run the complete Rust-owned supervisor lifecycle inside the caller's Tokio
/// runtime. `parent` distinguishes hosted execution from normal foreground
/// execution without adding a CLI surface.
pub async fn run_hosted(
    journal: &Path,
    options: SupervisorOptions,
    parent: Option<DeclaredParent>,
) -> SupervisorHostOutcome {
    let generation = match load_generation(journal) {
        Ok(generation) => generation,
        Err(reason) => return SupervisorHostOutcome::Refused { reason },
    };
    let journal_binary = match runtime::preflight_journal_binary(&options) {
        Ok(binary) => binary,
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::SiblingBinaryResolution(error.into()),
            };
        }
    };
    let admission = match SupervisorBootAdmission::acquire(journal) {
        Ok(admission) => admission,
        Err(LifecycleError::SyncConflict(_)) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::SyncConflict,
            };
        }
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::Lifecycle(LifecycleBootError::Failed(
                    error.to_string(),
                )),
            };
        }
    };
    let parent_watch = match parent {
        Some(parent) => match ParentWatch::admit(parent, &SystemProcessInstanceSource) {
            Ok(watch) => Some(watch),
            Err(error) => {
                return SupervisorHostOutcome::Refused {
                    reason: SupervisorBootRefusal::ParentLiveness(error),
                };
            }
        },
        None => None,
    };
    let lifecycle = match admission.activate() {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::Lifecycle(LifecycleBootError::Failed(
                    error.to_string(),
                )),
            };
        }
    };
    let admitted = HostedSupervisorAdmission {
        lifecycle,
        _generation: generation,
        parent_watch,
    };
    let outcome = match runtime::boot_and_tick(
        admitted.lifecycle,
        journal.to_path_buf(),
        options,
        journal_binary,
        admitted.parent_watch,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(runtime::RuntimeBootError::ParentLostBeforeReadiness(reason)) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::ParentLostBeforeReadiness(reason),
            };
        }
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::Lifecycle(LifecycleBootError::Failed(
                    error.to_string(),
                )),
            };
        }
    };

    let cause = match outcome.stop_reason {
        tick::SupervisorStopReason::Signal(tick::SupervisorSignal::SigTerm) => {
            ShutdownCause::Signal(SupervisorSignal::SigTerm)
        }
        tick::SupervisorStopReason::Signal(tick::SupervisorSignal::SigInt) => {
            ShutdownCause::Signal(SupervisorSignal::SigInt)
        }
        tick::SupervisorStopReason::SyncConflict => ShutdownCause::SyncConflict,
        tick::SupervisorStopReason::ParentLost(reason) => {
            let mut driver = outcome.state.into_shutdown_driver();
            let report = outcome
                .lifecycle
                .shutdown(&mut driver, outcome.regime, false)
                .ok();
            return SupervisorHostOutcome::ParentLost {
                reason,
                shutdown: report.map_or(ShutdownDisposition::Orderly, |report| report.disposition),
            };
        }
    };
    let sync_conflict = matches!(cause, ShutdownCause::SyncConflict);
    let mut driver = outcome.state.into_shutdown_driver();
    let report = outcome
        .lifecycle
        .shutdown(&mut driver, outcome.regime, sync_conflict)
        .ok();
    shutdown_outcome(cause, report)
}

fn shutdown_outcome(cause: ShutdownCause, report: Option<ShutdownReport>) -> SupervisorHostOutcome {
    if let Some(report) = report
        && matches!(
            report.disposition,
            ShutdownDisposition::ForcedAfterGraceTimeout
        )
    {
        return SupervisorHostOutcome::ForcedShutdownAfterGraceTimeout {
            cause,
            phase: report
                .forced_phase
                .unwrap_or(ShutdownPhase::StopChildrenCompleted),
        };
    }
    SupervisorHostOutcome::OrderlyShutdown { cause }
}

fn load_generation(journal: &Path) -> Result<Generation, SupervisorBootRefusal> {
    let home = std::env::var_os("HOME").ok_or(SupervisorBootRefusal::InstallationBinding(
        InstallationBindingRefusal::LoadFailed,
    ))?;
    let owner =
        crate::installation_context::owner_base_at_home(PathBuf::from(home)).map_err(|_| {
            SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed)
        })?;
    let root =
        crate::installation_context::identity_root_from_current_executable().map_err(|_| {
            SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed)
        })?;
    let root_token = root_token_from_path(&root).map_err(|_| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed)
    })?;
    let binding = load_installation_binding(&owner, &root_token).map_err(|_| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed)
    })?;
    let journal_token = journal_token_from_path(journal).map_err(|_| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed)
    })?;
    if binding.journal_token != journal_token {
        return Err(SupervisorBootRefusal::InstallationBinding(
            InstallationBindingRefusal::JournalTokenMismatch,
        ));
    }
    Ok(binding.generation)
}

impl From<runtime::JournalBinaryPreflightError> for SiblingBinaryResolutionError {
    fn from(value: runtime::JournalBinaryPreflightError) -> Self {
        match value {
            runtime::JournalBinaryPreflightError::CurrentExecutable => Self::CurrentExecutable,
            runtime::JournalBinaryPreflightError::MissingOrNotExecutable { path } => {
                Self::MissingOrNotExecutable { path }
            }
            runtime::JournalBinaryPreflightError::InvalidLayout => Self::InvalidLayout,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{ShutdownCause, SupervisorHostOutcome, SupervisorSignal, shutdown_outcome};
    use solstone_core_system::lifecycle::{ShutdownDisposition, ShutdownPhase, ShutdownReport};

    #[test]
    fn forced_shutdown_report_remains_a_distinct_host_outcome() {
        let report = ShutdownReport {
            phases: Vec::new(),
            disposition: ShutdownDisposition::ForcedAfterGraceTimeout,
            forced_phase: Some(ShutdownPhase::StopChildrenCompleted),
        };
        assert_eq!(
            shutdown_outcome(
                ShutdownCause::Signal(SupervisorSignal::SigTerm),
                Some(report)
            ),
            SupervisorHostOutcome::ForcedShutdownAfterGraceTimeout {
                cause: ShutdownCause::Signal(SupervisorSignal::SigTerm),
                phase: ShutdownPhase::StopChildrenCompleted,
            }
        );
    }
}
