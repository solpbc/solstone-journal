// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! [check] Lifecycle termination classification: the hosted supervisor route, AppService (including
//! restart signalling), and TaskQueue are supervisor-owned exact-instance routes; they use birth-validated
//! direct-PID signalling and must not use process-group fallback. provider_runtime launch/parakeet and
//! retention-client retain the legacy terminate() route and are out of scope for this lifecycle contract.
//! ParentDeathBackstop is deleted; it has no remaining production route. Any new caller must be classified
//! here before selecting exact or legacy termination.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_cli::SupervisorOptions;
use solstone_core_installation_identity::{
    Generation, journal_token_from_path, load_installation_binding, root_token_from_path,
};
use solstone_core_system::lifecycle::{
    ArtifactClearOutcome, DeclaredParent, LifecycleError, ParentAdmissionFailure, ParentLossReason,
    ParentWatch, ShutdownDisposition, ShutdownOutcome, ShutdownPhase, SupervisorBootAdmission,
    SyncTickOutcome,
};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system_health::format_sync_scan_failure_copy;

use super::{runtime, tick};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorSignal {
    SigTerm,
    SigInt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SyncFailureKind {
    Conflict,
    RenewalFailure,
    CompleteScanFailure,
    RetainedObservationFailure,
}

impl SyncFailureKind {
    fn classify(outcome: &SyncTickOutcome) -> Self {
        match outcome {
            SyncTickOutcome::Healthy => {
                unreachable!("healthy sync ticks never stop the supervisor loop")
            }
            SyncTickOutcome::Conflict(_) => Self::Conflict,
            SyncTickOutcome::RenewalFailure(_) => Self::RenewalFailure,
            SyncTickOutcome::CompleteScanFailure(_) => Self::CompleteScanFailure,
            SyncTickOutcome::RetainedObservationFailure(_) => Self::RetainedObservationFailure,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShutdownCause {
    Signal(SupervisorSignal),
    Sync(SyncFailureKind),
    ParentLost(ParentLossReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SiblingBinaryResolutionError {
    CurrentExecutable,
    MissingOrNotExecutable { path: PathBuf },
    InvalidLayout,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallationBindingRefusal {
    LoadFailed,
    JournalTokenMismatch,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleBootError {
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorBootRefusal {
    SyncConflict,
    /// The pre-formatted, terminal-safe copy for an unsafe or incomplete sync
    /// scan at boot, produced by `format_sync_scan_failure_copy`.
    SyncScan(String),
    ParentLiveness(ParentAdmissionFailure),
    ParentLostBeforeReadiness(ParentLossReason),
    SiblingBinaryResolution(SiblingBinaryResolutionError),
    InstallationBinding(InstallationBindingRefusal),
    Lifecycle(LifecycleBootError),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    LifecycleShutdownFailed {
        cause: ShutdownCause,
        readiness: ArtifactClearOutcome,
        self_heartbeat: ArtifactClearOutcome,
        identity: ArtifactClearOutcome,
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
        Err(LifecycleError::SyncScan(failure)) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::SyncScan(format_sync_scan_failure_copy(&failure)),
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
        tick::SupervisorStopReason::Sync(sync_outcome) => {
            ShutdownCause::Sync(SyncFailureKind::classify(&sync_outcome))
        }
        tick::SupervisorStopReason::ParentLost(reason) => ShutdownCause::ParentLost(reason),
    };
    let sync_conflict = matches!(cause, ShutdownCause::Sync(SyncFailureKind::Conflict));
    let mut driver = outcome.state.into_shutdown_driver();
    let shutdown = outcome
        .lifecycle
        .shutdown(&mut driver, outcome.regime, sync_conflict);
    classify_shutdown(cause, shutdown)
}

fn classify_shutdown(cause: ShutdownCause, outcome: ShutdownOutcome) -> SupervisorHostOutcome {
    let ShutdownOutcome {
        report,
        readiness,
        self_heartbeat,
        identity,
    } = outcome;
    if matches!(&readiness, ArtifactClearOutcome::Failed(_))
        || matches!(&self_heartbeat, ArtifactClearOutcome::Failed(_))
        || matches!(&identity, ArtifactClearOutcome::Failed(_))
    {
        return SupervisorHostOutcome::LifecycleShutdownFailed {
            cause,
            readiness,
            self_heartbeat,
            identity,
        };
    }
    if let ShutdownCause::ParentLost(reason) = cause {
        return SupervisorHostOutcome::ParentLost {
            reason,
            shutdown: report.disposition,
        };
    }
    if matches!(
        report.disposition,
        ShutdownDisposition::ForcedAfterGraceTimeout
    ) {
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
    use tempfile::tempdir;

    use super::super::receipt::{read_hosted_supervisor_receipt, write_hosted_supervisor_receipt};
    use super::{
        ShutdownCause, SupervisorHostOutcome, SupervisorSignal, SyncFailureKind, classify_shutdown,
    };
    use solstone_core_system::lifecycle::{
        ArtifactClearOutcome, DeclaredParent, ParentLossReason, ParentWatch, ParentWatchStatus,
        ShutdownDisposition, ShutdownOutcome, ShutdownPhase, ShutdownReport,
    };
    use solstone_core_system::process::{
        ExecutionState, InspectResult, InstanceCensus, ProcessBirth, ProcessInstance,
        ProcessInstanceSource,
    };

    struct Source {
        self_result: InspectResult,
        parent_result: InspectResult,
    }

    impl ProcessInstanceSource for Source {
        fn inspect(&self, pid: u32) -> InspectResult {
            if pid == std::process::id() {
                self.self_result
            } else {
                self.parent_result
            }
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Incomplete(Vec::new())
        }
    }

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn present(instance: ProcessInstance, ppid: Option<u32>) -> InspectResult {
        InspectResult::Present {
            instance,
            execution: ExecutionState::Running,
            ppid,
            pgid: None,
        }
    }

    fn shutdown_outcome(report: ShutdownReport) -> ShutdownOutcome {
        ShutdownOutcome {
            report,
            readiness: ArtifactClearOutcome::Cleared,
            self_heartbeat: ArtifactClearOutcome::Cleared,
            identity: ArtifactClearOutcome::Cleared,
        }
    }

    fn failed_shutdown_outcome(
        readiness: ArtifactClearOutcome,
        self_heartbeat: ArtifactClearOutcome,
        identity: ArtifactClearOutcome,
    ) -> ShutdownOutcome {
        ShutdownOutcome {
            report: ShutdownReport::default(),
            readiness,
            self_heartbeat,
            identity,
        }
    }

    #[test]
    fn forced_shutdown_report_remains_a_distinct_host_outcome() {
        let outcome = shutdown_outcome(ShutdownReport {
            phases: Vec::new(),
            disposition: ShutdownDisposition::ForcedAfterGraceTimeout,
            forced_phase: Some(ShutdownPhase::StopChildrenCompleted),
        });
        assert_eq!(
            classify_shutdown(ShutdownCause::Signal(SupervisorSignal::SigTerm), outcome),
            SupervisorHostOutcome::ForcedShutdownAfterGraceTimeout {
                cause: ShutdownCause::Signal(SupervisorSignal::SigTerm),
                phase: ShutdownPhase::StopChildrenCompleted,
            }
        );
    }

    #[test]
    fn cleanup_failures_dominate_every_post_ready_cause() {
        let cases = [
            (
                ShutdownCause::ParentLost(ParentLossReason::ExitedOrReused),
                failed_shutdown_outcome(
                    ArtifactClearOutcome::Failed("readiness".into()),
                    ArtifactClearOutcome::Cleared,
                    ArtifactClearOutcome::Cleared,
                ),
            ),
            (
                ShutdownCause::Signal(SupervisorSignal::SigTerm),
                failed_shutdown_outcome(
                    ArtifactClearOutcome::Cleared,
                    ArtifactClearOutcome::Failed("heartbeat".into()),
                    ArtifactClearOutcome::Cleared,
                ),
            ),
            (
                ShutdownCause::Sync(SyncFailureKind::RenewalFailure),
                failed_shutdown_outcome(
                    ArtifactClearOutcome::Cleared,
                    ArtifactClearOutcome::Cleared,
                    ArtifactClearOutcome::Failed("identity".into()),
                ),
            ),
            (
                ShutdownCause::Sync(SyncFailureKind::Conflict),
                failed_shutdown_outcome(
                    ArtifactClearOutcome::Failed("readiness".into()),
                    ArtifactClearOutcome::Skipped,
                    ArtifactClearOutcome::Skipped,
                ),
            ),
        ];

        for (cause, outcome) in cases {
            assert!(matches!(
                classify_shutdown(cause, outcome),
                SupervisorHostOutcome::LifecycleShutdownFailed { .. }
            ));
        }
    }

    #[test]
    fn parent_loss_stays_distinct_when_shutdown_is_forced() {
        let outcome = shutdown_outcome(ShutdownReport {
            phases: Vec::new(),
            disposition: ShutdownDisposition::ForcedAfterGraceTimeout,
            forced_phase: Some(ShutdownPhase::StopChildrenCompleted),
        });

        assert_eq!(
            classify_shutdown(
                ShutdownCause::ParentLost(ParentLossReason::ExitedOrReused),
                outcome
            ),
            SupervisorHostOutcome::ParentLost {
                reason: ParentLossReason::ExitedOrReused,
                shutdown: ShutdownDisposition::ForcedAfterGraceTimeout,
            }
        );
    }

    #[test]
    fn parent_loss_receipts_replace_stale_outcomes_for_lost_parent_observations() {
        let expected_parent = instance(42, 10);
        let admitted_source = Source {
            self_result: present(instance(std::process::id(), 1), Some(expected_parent.pid)),
            parent_result: present(expected_parent, Some(1)),
        };
        let watch = ParentWatch::admit(
            DeclaredParent::from_instance(expected_parent),
            &admitted_source,
        )
        .expect("parent admitted");

        let temporary = tempdir().expect("temporary receipt directory");
        let receipt_path = temporary.path().join("hosted.outcome");
        let stale = SupervisorHostOutcome::OrderlyShutdown {
            cause: ShutdownCause::Signal(SupervisorSignal::SigTerm),
        };
        write_hosted_supervisor_receipt(&receipt_path, "stale-nonce", &stale)
            .expect("stale receipt");
        assert_eq!(
            read_hosted_supervisor_receipt(&receipt_path)
                .expect("stale receipt reads")
                .nonce,
            "stale-nonce"
        );

        let cases = [
            (
                Source {
                    self_result: InspectResult::Unverifiable,
                    parent_result: present(instance(expected_parent.pid, 11), Some(1)),
                },
                ParentLossReason::ExitedOrReused,
            ),
            (
                Source {
                    self_result: InspectResult::Unverifiable,
                    parent_result: InspectResult::Unverifiable,
                },
                ParentLossReason::Unverifiable,
            ),
        ];
        for (index, (source, reason)) in cases.into_iter().enumerate() {
            assert_eq!(watch.check(&source), ParentWatchStatus::Lost(reason));
            let outcome = classify_shutdown(
                ShutdownCause::ParentLost(reason),
                shutdown_outcome(ShutdownReport::default()),
            );
            assert_eq!(
                outcome,
                SupervisorHostOutcome::ParentLost {
                    reason,
                    shutdown: ShutdownDisposition::Orderly,
                }
            );

            let nonce = format!("parent-loss-{index}");
            write_hosted_supervisor_receipt(&receipt_path, &nonce, &outcome)
                .expect("parent-loss receipt");
            let receipt =
                read_hosted_supervisor_receipt(&receipt_path).expect("parent-loss receipt reads");
            assert_eq!(receipt.nonce, nonce);
            assert_eq!(receipt.outcome, outcome);
        }
    }
}
