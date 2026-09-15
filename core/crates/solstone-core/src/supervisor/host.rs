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

fn lifecycle_boot_refusal(error: LifecycleError) -> SupervisorBootRefusal {
    match error {
        LifecycleError::SyncScan(failure) => {
            SupervisorBootRefusal::SyncScan(format_sync_scan_failure_copy(&failure))
        }
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
/// ⛔ The recovery block is gated to the ONE cause it actually fixes. Only
/// `InitialAdmissionHandshake` reads the parent-loss records; the other three
/// are launch, identity and stop-confirmation failures that never touch them,
/// and offering the same `mv` for those leaves the owner re-running into an
/// identical error with a stray directory beside their journal.
///
/// ⚠ The two arms close differently on purpose. "untouched" is only true where
/// nothing inside the journal was named, and the gated arm prints a path under
/// `health/` -- there the reassurance has to be about that record instead.
fn format_lifecycle_recovery_copy(
    journal: &Path,
    reason: runtime::ParentLossCoordinatorBootstrapFailure,
) -> String {
    let mut copy = format!("this start could not continue.\n\ndetails: {reason}\n");
    if matches!(
        reason,
        runtime::ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake
    ) {
        let records = journal.join("health/parent-loss");
        // ⚠ A fixed `.set-aside` destination silently NESTS on a second run:
        // once `parent-loss.set-aside` exists, `mv parent-loss
        // parent-loss.set-aside` puts the record *inside* it and reports
        // nothing. Stamping the destination keeps every refusal's third line
        // new, so a repeat either works or fails out loud.
        let set_aside = format!(
            "{}.set-aside-{}",
            records.display(),
            chrono::Local::now().format("%Y%m%d-%H%M%S")
        );
        // ⚠ The stop comes first and is not optional: this refusal exits
        // TEMPFAIL and the installed unit restarts on failure, so without it the
        // service is starting again every few seconds while the owner types.
        #[cfg(target_os = "macos")]
        let stop = "launchctl bootout gui/$(id -u)/org.solpbc.solstone";
        #[cfg(not(target_os = "macos"))]
        let stop = "systemctl --user stop solstone.service";
        // ⛔ `journal up`, never `journal start`. `journal start` runs the
        // supervisor in the FOREGROUND and does not touch the service, so it
        // would leave the owner holding a process tied to that terminal with
        // the unit line 1 just stopped still down. `journal up` is the alias
        // for `journal service start`, and the inverse of the stop above.
        copy.push_str(&format!(
            "\nan earlier start may have left a record behind. to move it aside, open a \
             terminal and run these three lines:\n\
             \x20   {stop}\n\
             \x20   mv {} {set_aside}\n\
             \x20   journal up\n\
             \nnone of this holds your memories.\n",
            records.display()
        ));
    } else {
        // ⚠ This is the settled close for the family --
        // ADMISSION_WAIT_{TERMINAL,ACTIVE,UNVERIFIABLE}_COPY all end on exactly
        // "wait a moment, then try again." The wait is load-bearing on
        // `CoordinatorRetirementUnverified`, where retrying at once re-enters
        // the race that produced the refusal.
        //
        // ⛔ No demonstrative in the closing line here: these arms print no
        // path, so "none of this holds your memories" would point at nothing
        // and its most available reading is the opposite of the intent.
        copy.push_str("\nwait a moment, then try again.\n\nyour journal is untouched.\n");
    }
    copy
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
    /// Four properties, each of which an ordinary edit breaks silently:
    ///   1. every variant hands the owner something to try. The recovery block
    ///      is gated to the one cause it actually fixes, so without a fallback
    ///      the other three end on a bare diagnosis -- the only refusals in
    ///      this family that would.
    ///   2. no arm sends the owner to `journal start`, which starts the
    ///      supervisor in the foreground rather than the service.
    ///   3. only the gated cause names the ledger path.
    ///   4. "untouched" appears only where no path inside the journal does.
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

        let journal = std::path::Path::new("/home/owner/journal");
        for reason in ALL {
            let copy = super::format_lifecycle_recovery_copy(journal, reason);
            let gated = reason == Failure::InitialAdmissionHandshake;

            assert!(
                copy.contains("journal up") || copy.contains("wait a moment, then try again."),
                "{reason:?} leaves the owner with nothing to try:\n{copy}"
            );

            assert!(
                !copy.contains("journal start"),
                "{reason:?} sends the owner to `journal start`, which runs the supervisor in the \
                 foreground and leaves the service it just stopped down. The owner-facing start \
                 is `journal up`:\n{copy}"
            );

            assert_eq!(
                copy.contains("health/parent-loss"),
                gated,
                "{reason:?} may name the parent-loss records only if it is the cause that reads \
                 them -- every other arm would send the owner to move a directory that is not \
                 the problem:\n{copy}"
            );

            // ⚠ The asymmetry is the point. Claiming the journal is untouched
            // while printing a path under `health/` is read against the `mv`
            // directly above it, and is false there.
            assert_eq!(
                copy.contains("untouched"),
                !gated,
                "{reason:?} must reassure about the record it named, not about the journal it \
                 just told the owner to reach into:\n{copy}"
            );
        }
    }

    /// The stop has to precede the `mv`. This refusal exits TEMPFAIL under a
    /// `Restart=on-failure` unit, so a set-aside performed while the service is
    /// still cycling is re-created by the next start before the owner finishes
    /// typing -- and the copy reads as simply not working.
    #[test]
    fn admission_recovery_copy_stops_the_service_before_setting_the_record_aside() {
        let copy = super::format_lifecycle_recovery_copy(
            std::path::Path::new("/home/owner/journal"),
            super::runtime::ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake,
        );

        let stop = copy
            .find("stop solstone.service")
            .or_else(|| copy.find("bootout"))
            .expect("the recovery copy names a platform stop command");
        let set_aside = copy
            .find("mv ")
            .expect("the recovery copy names the set-aside");

        assert!(
            stop < set_aside,
            "the stop must come before the set-aside:\n{copy}"
        );
    }

    /// ⚠ `mv a a.set-aside` moves `a` INSIDE `a.set-aside` once that directory
    /// exists, silently, which is exactly what an owner running these lines a
    /// second time does. The destination carries a stamp so every refusal names
    /// somewhere new and a repeat cannot nest.
    #[test]
    fn admission_recovery_set_aside_destination_cannot_nest_on_a_second_run() {
        let copy = super::format_lifecycle_recovery_copy(
            std::path::Path::new("/home/owner/journal"),
            super::runtime::ParentLossCoordinatorBootstrapFailure::InitialAdmissionHandshake,
        );

        assert!(
            copy.contains("health/parent-loss.set-aside-"),
            "the set-aside destination must be stamped:\n{copy}"
        );
        assert!(
            !copy.contains("health/parent-loss.set-aside\n"),
            "a bare `.set-aside` destination nests on the owner's second run:\n{copy}"
        );
    }
}
