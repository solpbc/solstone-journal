// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! [check] Lifecycle termination classification: the hosted supervisor route, AppService (including
//! restart signalling), and TaskQueue are supervisor-owned exact-instance routes; they use birth-validated
//! direct-PID signalling and must not use process-group fallback. provider_runtime launch/parakeet and
//! retention-client retain the legacy terminate() route and are out of scope for this lifecycle contract.
//! ParentDeathBackstop is deleted; it has no remaining production route. Any new caller must be classified
//! here before selecting exact or legacy termination.

use std::fmt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use solstone_core_cli::SupervisorOptions;
use solstone_core_installation_identity::{
    Generation, IdentityError, journal_token_from_path, load_installation_binding,
    root_token_from_path,
};
use solstone_core_journal_io::legacy_log_alias::cleanup_legacy_log_aliases;
#[cfg(unix)]
use solstone_core_system::lifecycle::{
    ADMISSION_WAIT_ACTIVE_COPY, AdmissionWaitTerminalReason, SupervisorBootAdmission,
};
use solstone_core_system::lifecycle::{
    ArtifactClearOutcome, DeclaredParent, LifecycleError, ParentAdmissionFailure, ParentLossReason,
    ParentWatch, ShutdownDisposition, ShutdownOutcome, ShutdownPhase, SyncTickOutcome, WriterId,
};
#[cfg(unix)]
use solstone_core_system::lifecycle::{ParentLossReaderOutcome, read_parent_loss_outcome};
use solstone_core_system::process::SystemProcessInstanceSource;
use solstone_core_system_health::format_sync_scan_failure_copy;
use solstone_core_transcribe::{SpeakersAnalyzeGeneration, SpeakersAnalyzeOwnerRole};

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
    StaleHeartbeatCollectionFailure,
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
            SyncTickOutcome::StaleHeartbeatCollectionFailure(_) => {
                Self::StaleHeartbeatCollectionFailure
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ShutdownCause {
    Signal(SupervisorSignal),
    Sync(SyncFailureKind),
    ParentLost(ParentLossReason),
    /// The host session is ending: Windows told the retained forwarder, which
    /// asked this supervisor to stop. A requested stop, not a failure.
    HostSessionEnd,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SiblingBinaryResolutionError {
    CurrentExecutable,
    MissingOrNotExecutable { path: PathBuf },
    InvalidLayout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum InstallationBindingRefusal {
    LoadFailed(String),
    JournalTokenMismatch,
}

impl fmt::Display for InstallationBindingRefusal {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let detail = match self {
            Self::LoadFailed(detail) => detail.as_str(),
            Self::JournalTokenMismatch => {
                "the saved installation binding is for a different journal"
            }
        };
        formatter.write_str(&crate::installation_context::installation_recovery_copy(
            detail,
        ))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LifecycleBootError {
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SupervisorBootRefusal {
    /// Pre-formatted, terminal-safe owner copy for a sync admission refusal.
    SyncScan(String),
    /// A supervisor for this journal already holds the singleton lock. Not a
    /// fault: the copy says so plainly (`ALREADY_RUNNING_COPY`).
    AlreadyRunning,
    ParentLiveness(ParentAdmissionFailure),
    ParentLostBeforeReadiness(ParentLossReason),
    SiblingBinaryResolution(SiblingBinaryResolutionError),
    InstallationBinding(InstallationBindingRefusal),
    AdmissionWaitTerminal,
    AdmissionWaitUnverifiable,
    Lifecycle(LifecycleBootError),
    /// Pre-formatted, terminal-safe owner copy for a failed speakers-analyze
    /// installation-generation acquisition. Distinct from `_generation`
    /// (installation-binding) and the parent-loss lifecycle generation.
    SpeakersAnalyzeGeneration(String),
    /// Pre-formatted, terminal-safe owner copy for a lifecycle bookkeeping
    /// generation that cannot be resolved and therefore blocks every boot.
    LifecycleRecovery(String),
    /// The retired-alias convergence pass could not safely inspect the journal.
    LegacyLogCleanup(String),
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
    #[cfg(unix)]
    lifecycle: solstone_core_system::lifecycle::PreReadySupervisorLifecycle,
    #[cfg(windows)]
    lifecycle: solstone_core_system::lifecycle::SupervisorLifecycle,
    _generation: Generation,
    _speakers_analyze_generation: SpeakersAnalyzeGeneration,
    parent_watch: Option<ParentWatch>,
}

struct HostedInstallationBinding {
    #[cfg(windows)]
    guard: solstone_core_installation_identity::GuardFields,
    generation: Generation,
    writer_id: WriterId,
}

/// Owner copy for a start that found this journal already running. Exits
/// TEMPFAIL like its siblings, but there is nothing to retry: the running
/// one is the journal.
pub const ALREADY_RUNNING_COPY: &str = "your journal is already running.\n\nthis start stepped aside so the running one is not disturbed. nothing needs doing.\n";

fn lifecycle_boot_refusal(error: LifecycleError) -> SupervisorBootRefusal {
    match error {
        LifecycleError::SyncScan(failure) => {
            SupervisorBootRefusal::SyncScan(format_sync_scan_failure_copy(&failure))
        }
        LifecycleError::AlreadyRunning => SupervisorBootRefusal::AlreadyRunning,
        #[cfg(unix)]
        LifecycleError::AdmissionWaitTerminal(AdmissionWaitTerminalReason::ActivityRemains) => {
            SupervisorBootRefusal::AdmissionWaitTerminal
        }
        LifecycleError::AdmissionWaitMarkerLive => {
            #[cfg(unix)]
            {
                SupervisorBootRefusal::SyncScan(ADMISSION_WAIT_ACTIVE_COPY.to_owned())
            }
            #[cfg(not(unix))]
            {
                SupervisorBootRefusal::AdmissionWaitUnverifiable
            }
        }
        #[cfg(unix)]
        LifecycleError::AdmissionWaitTerminal(AdmissionWaitTerminalReason::ClockDiscontinuity) => {
            SupervisorBootRefusal::AdmissionWaitUnverifiable
        }
        #[cfg(unix)]
        LifecycleError::AdmissionWaitMarkerNeedsAttention(_)
        | LifecycleError::AdmissionWaitMarkerCleanup(_) => {
            SupervisorBootRefusal::AdmissionWaitUnverifiable
        }
        LifecycleError::AdmissionHeartbeatNeedsAttention { .. }
        | LifecycleError::AdmissionWaitProcessIdentity
        | LifecycleError::AdmissionWaitMarkerPublication(_)
        | LifecycleError::PostPublicationHeartbeatCleanup(_) => {
            SupervisorBootRefusal::AdmissionWaitUnverifiable
        }
        error => SupervisorBootRefusal::Lifecycle(LifecycleBootError::Failed(error.to_string())),
    }
}

/// Owner copy for a start that could not establish its lifecycle authority.
///
/// Shape follows the sibling refusals: headline, then what to do, then the
/// reassurance, and the technical `details:` label LAST
/// (`installation_context::installation_recovery_copy`,
/// `system_health::sync_copy`). The owner's second line has to be help.
///
/// ⛔ There is no manual step any more. A generation whose supervisor and
/// coordinator both exited is closed by the next start on its own
/// (`lifecycle::parent_loss_closure`), so the walkthrough this copy used to
/// carry -- `mv health/parent-loss …` -- would set aside the very records that
/// closure retires from. Every remaining cause converges on a retry: a missed
/// deadline under load, a coordinator still adjudicating the last run, or a
/// process from the last run still exiting.
fn format_lifecycle_recovery_copy(
    journal: &Path,
    reason: runtime::ParentLossCoordinatorBootstrapFailure,
) -> String {
    let mut copy = String::from("this start could not continue.\n");
    // ⚠ The settled close for this family --
    // ADMISSION_WAIT_{TERMINAL,ACTIVE,UNVERIFIABLE}_COPY all end on exactly
    // "wait a moment, then try again." The wait is load-bearing on
    // `CoordinatorRetirementUnverified`, where retrying at once re-enters the
    // race that produced the refusal.
    if last_run_is_still_closing(journal, reason) {
        copy.push_str("\nthe last run is still shutting down. wait a moment, then try again.\n");
    } else {
        copy.push_str("\nwait a moment, then try again.\n");
    }
    // ⛔ "your memories", NOT "your journal is untouched". Every one of these
    // paths may have written bookkeeping under `health/`, and
    // `CoordinatorRetirementUnverified` is returned precisely when a helper
    // that may still be RUNNING could not be confirmed stopped -- so the one
    // arm named for its lack of confirmation is the one that would be
    // asserting a confirmed fact about the journal.
    copy.push_str("\nyour memories are untouched.\n");
    copy.push_str(&format!("\ndetails: {reason}\n"));
    copy
}

/// The one refusal cause the owner can be told more about: the coordinator
/// did not answer because the previous run's coordinator is still live and
/// adjudicating, which resolves itself within its retirement deadline. Read
/// off the ledger, never inferred from the variant alone.
#[cfg(unix)]
fn last_run_is_still_closing(
    journal: &Path,
    reason: runtime::ParentLossCoordinatorBootstrapFailure,
) -> bool {
    reason == runtime::ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake
        && matches!(
            read_parent_loss_outcome(journal),
            Ok(ParentLossReaderOutcome::Active { .. })
        )
}

#[cfg(not(unix))]
fn last_run_is_still_closing(
    _journal: &Path,
    _reason: runtime::ParentLossCoordinatorBootstrapFailure,
) -> bool {
    false
}

/// Run the complete Rust-owned supervisor lifecycle inside the caller's Tokio
/// runtime. `parent` distinguishes hosted execution from normal foreground
/// execution without adding a CLI surface.
pub async fn run_hosted(
    journal: &Path,
    options: SupervisorOptions,
    parent: Option<DeclaredParent>,
    #[cfg(windows)] installed_task: Option<
        &solstone_core_system::process::AdmittedInstalledTaskLaunch,
    >,
) -> SupervisorHostOutcome {
    let binding = match load_generation(journal) {
        Ok(binding) => binding,
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
    // Acquired before any supervisor lifecycle admission artifact exists
    // (heartbeat, readiness, parent-loss state): a refusal here must leave
    // zero such artifacts, so this must happen strictly before
    // `SupervisorBootAdmission::acquire`.
    let speakers_analyze_generation =
        match solstone_core_transcribe::enter_speakers_analyze_generation(
            journal,
            SpeakersAnalyzeOwnerRole::Supervisor,
            #[cfg(windows)]
            None,
        ) {
            Ok(generation) => generation,
            Err(error) => {
                return SupervisorHostOutcome::Refused {
                    reason: SupervisorBootRefusal::SpeakersAnalyzeGeneration(
                        error.message().unwrap_or_default().to_owned(),
                    ),
                };
            }
        };
    #[cfg(unix)]
    let admission = match SupervisorBootAdmission::acquire(journal, binding.writer_id.clone()) {
        Ok(admission) => admission,
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: lifecycle_boot_refusal(error),
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
    if let Err(error) = cleanup_legacy_log_aliases(journal) {
        return SupervisorHostOutcome::Refused {
            reason: SupervisorBootRefusal::LegacyLogCleanup(error.to_string()),
        };
    }
    #[cfg(unix)]
    let lifecycle = match admission.activate() {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: lifecycle_boot_refusal(error),
            };
        }
    };
    #[cfg(windows)]
    let lifecycle = match solstone_core_system::lifecycle::boot(journal, binding.writer_id.clone())
    {
        Ok(lifecycle) => lifecycle,
        Err(error) => {
            return SupervisorHostOutcome::Refused {
                reason: lifecycle_boot_refusal(error),
            };
        }
    };
    let sense_child_environment = speakers_analyze_generation.child_launch_context();
    let admitted = HostedSupervisorAdmission {
        lifecycle,
        _generation: binding.generation,
        _speakers_analyze_generation: speakers_analyze_generation,
        parent_watch,
    };
    let outcome = match runtime::boot_and_tick(
        admitted.lifecycle,
        journal.to_path_buf(),
        options,
        journal_binary,
        admitted.parent_watch,
        sense_child_environment,
        #[cfg(windows)]
        binding.guard,
        #[cfg(windows)]
        installed_task,
    )
    .await
    {
        Ok(outcome) => outcome,
        Err(runtime::RuntimeBootError::ParentLostBeforeReadiness(reason)) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::ParentLostBeforeReadiness(reason),
            };
        }
        Err(runtime::RuntimeBootError::SyncScan(failure)) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::SyncScan(format_sync_scan_failure_copy(&failure)),
            };
        }
        Err(runtime::RuntimeBootError::AdmissionWaitTerminal) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::AdmissionWaitTerminal,
            };
        }
        Err(runtime::RuntimeBootError::BootstrapRecoveryRequired(reason)) => {
            return SupervisorHostOutcome::Refused {
                reason: SupervisorBootRefusal::LifecycleRecovery(format_lifecycle_recovery_copy(
                    journal, reason,
                )),
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
        #[cfg(windows)]
        tick::SupervisorStopReason::HostSessionEnd => ShutdownCause::HostSessionEnd,
    };
    let sync_conflict = matches!(cause, ShutdownCause::Sync(SyncFailureKind::Conflict));
    let mut driver = outcome.state.into_shutdown_driver(outcome.regime);
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

fn load_generation(journal: &Path) -> Result<HostedInstallationBinding, SupervisorBootRefusal> {
    #[cfg(unix)]
    let owner = {
        let home = std::env::var_os("HOME").ok_or_else(|| {
            SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(
                format!("home: {}", IdentityError::InvalidInput("HOME is not set")),
            ))
        })?;
        crate::installation_context::owner_base_at_home(PathBuf::from(home)).map_err(|error| {
            SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(
                format!("owner storage: {error}"),
            ))
        })?
    };
    #[cfg(windows)]
    let owner = solstone_core_installation_identity::owner_base().map_err(|error| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(format!(
            "owner storage: {error}"
        )))
    })?;
    let root =
        crate::installation_context::identity_root_from_current_executable().map_err(|error| {
            SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(
                format!("installation root: {error}"),
            ))
        })?;
    let root_token = root_token_from_path(&root).map_err(|error| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(format!(
            "root token: {error}"
        )))
    })?;
    let binding = load_installation_binding(&owner, &root_token).map_err(|error| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(format!(
            "saved binding: {error}"
        )))
    })?;
    let journal_token = journal_token_from_path(journal).map_err(|error| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(format!(
            "journal token: {error}"
        )))
    })?;
    if binding.journal_token != journal_token {
        return Err(SupervisorBootRefusal::InstallationBinding(
            InstallationBindingRefusal::JournalTokenMismatch,
        ));
    }
    let writer_id = WriterId::parse(&binding.id.as_hex()).map_err(|_| {
        SupervisorBootRefusal::InstallationBinding(InstallationBindingRefusal::LoadFailed(
            "the saved installation binding could not be loaded".to_owned(),
        ))
    })?;
    Ok(HostedInstallationBinding {
        #[cfg(windows)]
        guard: solstone_core_installation_identity::GuardFields::from_binding(&binding),
        generation: binding.generation,
        writer_id,
    })
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
    use std::io;

    use solstone_core_installation_identity::IdentityError;
    use tempfile::tempdir;

    use super::super::receipt::{read_hosted_supervisor_receipt, write_hosted_supervisor_receipt};
    #[cfg(unix)]
    use super::lifecycle_boot_refusal;
    use super::{
        InstallationBindingRefusal, ShutdownCause, SupervisorBootRefusal, SupervisorHostOutcome,
        SupervisorSignal, SyncFailureKind, classify_shutdown,
    };
    #[cfg(unix)]
    use solstone_core_system::lifecycle::{
        ADMISSION_WAIT_ACTIVE_COPY, AdmissionWaitTerminalReason, LifecycleError,
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
            uid: 501,
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
    fn parent_loss_receipts_replace_missing_stale_and_wrong_nonce_outcomes() {
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
        for (observation_index, (source, reason)) in cases.into_iter().enumerate() {
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

            for (receipt_state, previous) in [
                ("missing", None),
                ("stale", Some(("stale-nonce", stale.clone()))),
                ("wrong-nonce", Some(("unrelated-nonce", stale.clone()))),
            ] {
                let _ = std::fs::remove_file(&receipt_path);
                if let Some((nonce, previous_outcome)) = previous {
                    write_hosted_supervisor_receipt(&receipt_path, nonce, &previous_outcome)
                        .expect("previous receipt");
                } else {
                    assert!(!receipt_path.exists(), "{receipt_state} receipt is absent");
                }

                let nonce = format!("parent-loss-{observation_index}-{receipt_state}");
                write_hosted_supervisor_receipt(&receipt_path, &nonce, &outcome)
                    .expect("fresh parent-loss receipt");
                let receipt = read_hosted_supervisor_receipt(&receipt_path)
                    .expect("fresh parent-loss receipt reads");
                assert_eq!(receipt.nonce, nonce);
                assert_eq!(receipt.outcome, outcome);
            }
        }
    }

    #[test]
    fn installation_binding_refusal_uses_the_shared_recovery_copy() {
        assert_eq!(
            InstallationBindingRefusal::LoadFailed(
                "saved binding: namespace record is missing".into()
            )
            .to_string(),
            crate::installation_context::installation_recovery_copy(
                "saved binding: namespace record is missing"
            ),
        );
        assert_eq!(
            InstallationBindingRefusal::JournalTokenMismatch.to_string(),
            crate::installation_context::installation_recovery_copy(
                "the saved installation binding is for a different journal"
            )
        );
    }

    #[cfg(unix)]
    #[test]
    fn admission_wait_refusal_preserves_what_was_verified() {
        assert!(matches!(
            lifecycle_boot_refusal(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ActivityRemains
            )),
            SupervisorBootRefusal::AdmissionWaitTerminal
        ));
        assert!(matches!(
            lifecycle_boot_refusal(LifecycleError::AdmissionWaitTerminal(
                AdmissionWaitTerminalReason::ClockDiscontinuity
            )),
            SupervisorBootRefusal::AdmissionWaitUnverifiable
        ));
        assert!(matches!(
            lifecycle_boot_refusal(LifecycleError::AdmissionWaitMarkerLive),
            SupervisorBootRefusal::SyncScan(copy) if copy == ADMISSION_WAIT_ACTIVE_COPY
        ));
    }

    #[test]
    fn installation_root_permission_denied_prefix_is_format_only() {
        // A portable test cannot induce current_exe permission denial, so this covers formatting only.
        let error = io::Error::from(io::ErrorKind::PermissionDenied);
        assert_eq!(error.to_string(), "permission denied");
        assert_eq!(
            format!("installation root: {error}"),
            "installation root: permission denied"
        );
    }

    #[test]
    fn root_token_io_prefix_is_format_only() {
        // A portable test cannot deterministically race the resolved root away, so this covers formatting only.
        let temporary = tempdir().expect("temporary root-token directory");
        let missing = temporary.path().join("deleted-root");
        std::fs::create_dir(&missing).expect("create deleted root");
        std::fs::remove_dir(&missing).expect("delete root before canonicalize");
        let source = std::fs::canonicalize(&missing).expect_err("deleted root cannot canonicalize");
        let source_text = source.to_string();
        let error = IdentityError::Io {
            operation: "canonicalize root",
            source,
        };
        assert_eq!(
            format!("root token: {error}"),
            format!("root token: canonicalize root: {source_text}")
        );
    }

    #[test]
    fn root_token_overlength_prefix_is_format_only() {
        // A portable test cannot route an overlong resolved executable root, so this covers formatting only.
        let error = IdentityError::InvalidInput("path exceeds 4096 bytes");
        assert_eq!(
            format!("root token: {error}"),
            "root token: path exceeds 4096 bytes"
        );
    }

    #[test]
    fn installation_binding_refusals_round_trip_through_hosted_receipts() {
        let temporary = tempdir().expect("temporary receipt directory");
        let cases = [
            (
                "load-failed",
                InstallationBindingRefusal::LoadFailed("provider\\detail\n\u{1b}".into()),
            ),
            (
                "journal-mismatch",
                InstallationBindingRefusal::JournalTokenMismatch,
            ),
        ];

        for (nonce, refusal) in cases {
            let outcome = SupervisorHostOutcome::Refused {
                reason: super::SupervisorBootRefusal::InstallationBinding(refusal),
            };
            let before = match &outcome {
                SupervisorHostOutcome::Refused {
                    reason: super::SupervisorBootRefusal::InstallationBinding(refusal),
                } => refusal.to_string(),
                _ => unreachable!("fixture is an installation refusal"),
            };
            let path = temporary.path().join(format!("{nonce}.json"));
            write_hosted_supervisor_receipt(&path, nonce, &outcome).expect("write receipt");
            let receipt = read_hosted_supervisor_receipt(&path).expect("read receipt");
            assert_eq!(receipt.outcome, outcome);
            let after = match receipt.outcome {
                SupervisorHostOutcome::Refused {
                    reason: super::SupervisorBootRefusal::InstallationBinding(refusal),
                } => refusal.to_string(),
                _ => unreachable!("receipt retains installation refusal"),
            };
            assert_eq!(after, before);
            let before_details = before
                .rsplit_once("\ndetails: ")
                .expect("recovery display has details")
                .1;
            let after_details = after
                .rsplit_once("\ndetails: ")
                .expect("round-tripped recovery display has details")
                .1;
            assert_eq!(after_details, before_details);
        }
    }

    /// ⛔ A FALSIFICATION test for the refusal copy, not a snapshot of it.
    ///
    /// The properties, each of which an ordinary edit breaks silently:
    ///   1. every variant hands the owner something to try, and it is a retry:
    ///      there is no manual step, so no arm may send the owner to move,
    ///      rename or delete anything under their journal.
    ///   2. no arm sends the owner to `journal start`, which starts the
    ///      supervisor in the foreground rather than the service.
    ///   3. no arm names the ledger path: the next start closes an abandoned
    ///      generation itself, and the old walkthrough set aside the very
    ///      records that closure retires from.
    ///   4. every arm closes on "your memories", never on a claim that the
    ///      journal is untouched.
    ///   5. `details:` stays last, as the sibling refusals have it.
    #[test]
    fn every_lifecycle_recovery_copy_ends_in_something_the_owner_can_do() {
        use super::runtime::ParentLossCoordinatorBootstrapFailure as Failure;

        const ALL: [Failure; 4] = [
            Failure::Launch,
            Failure::IdentityEstablishment,
            Failure::InitialAdmissionHandshake,
            Failure::CoordinatorRetirementUnverified,
        ];
        // ⛔ Do not collapse this to `_ => {}`. It exists only so that adding a
        // fifth variant fails to COMPILE until it is added to ALL above, rather
        // than shipping an arm whose owner copy nobody ever read.
        match ALL[0] {
            Failure::Launch
            | Failure::IdentityEstablishment
            | Failure::InitialAdmissionHandshake
            | Failure::CoordinatorRetirementUnverified => {}
        }

        let home = tempdir().expect("tempdir");
        let journal = home.path();
        std::fs::create_dir_all(journal.join("health/parent-loss")).expect("records");

        for reason in ALL {
            let copy = super::format_lifecycle_recovery_copy(journal, reason);

            assert!(
                copy.contains("wait a moment, then try again."),
                "{reason:?} must offer the retry:\n{copy}"
            );
            for manual in [
                "mv ",
                "rm ",
                "rename",
                "move it aside",
                "health/parent-loss",
            ] {
                assert!(
                    !copy.contains(manual),
                    "{reason:?} hands the owner a manual step ({manual:?}); an abandoned \
                     generation is closed by the next start on its own:\n{copy}"
                );
            }
            assert!(
                !copy.contains("journal start"),
                "{reason:?} sends the owner to `journal start`, which runs the supervisor in the \
                 foreground and leaves the service it just stopped down:\n{copy}"
            );
            assert!(
                copy.contains("your memories are untouched"),
                "{reason:?} must close on the owner's memories:\n{copy}"
            );
            assert!(
                !copy.contains("your journal is untouched"),
                "{reason:?} asserts a confirmed fact about the journal that these paths cannot \
                 confirm:\n{copy}"
            );
            let details = copy.find("details:").expect("every arm carries details");
            assert!(
                copy[details..].trim_end().lines().count() == 1,
                "details: is the last line, as the sibling refusals have it:\n{copy}"
            );
        }
    }

    /// The "still shutting down" line is read off the ledger, never inferred
    /// from the variant: a journal with no lifecycle state at all gets the
    /// plain retry.
    #[test]
    fn a_journal_with_no_lifecycle_state_gets_the_plain_retry() {
        let home = tempdir().expect("tempdir");
        let copy = super::format_lifecycle_recovery_copy(
            home.path(),
            super::runtime::ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake,
        );
        assert!(
            !copy.contains("still shutting down"),
            "no ledger state means no claim about the last run:\n{copy}"
        );
        assert!(copy.contains("wait a moment, then try again."));
    }

    /// A coordinator that is genuinely live -- this test process, so the
    /// observation is real -- makes the handshake refusal say the last run is
    /// still shutting down, which is the one thing the owner can act on.
    #[cfg(unix)]
    #[test]
    fn a_live_coordinator_from_the_last_run_is_named_as_still_shutting_down() {
        use solstone_core_system::lifecycle::ParentLossLedger;
        use solstone_core_system::process::{
            InspectResult, ProcessBirth, ProcessInstance, ProcessInstanceSource,
            SystemProcessInstanceSource,
        };

        let home = tempdir().expect("tempdir");
        let ledger = ParentLossLedger::open(home.path()).expect("ledger");
        let supervisor = ProcessInstance {
            pid: 10,
            birth: ProcessBirth::linux(1, 1, 100),
        };
        let active = ledger
            .reserve_generation(supervisor, [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let live_coordinator = match SystemProcessInstanceSource.inspect(std::process::id()) {
            InspectResult::Present { instance, .. } => instance,
            _ => panic!("this test process must be observable"),
        };
        ledger
            .persist_coordinator_identity(active.generation, live_coordinator)
            .expect("coordinator identity");

        let copy = super::format_lifecycle_recovery_copy(
            home.path(),
            super::runtime::ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake,
        );
        assert!(
            copy.contains("the last run is still shutting down. wait a moment, then try again."),
            "a live coordinator must be named as the last run still closing:\n{copy}"
        );
    }
}
