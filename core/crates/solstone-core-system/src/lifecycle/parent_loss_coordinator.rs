// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! The single terminal authority for one hosted parent-loss generation.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    FileLease, LeaseError, LeaseOptions, LockOptions, acquire_file_lease, hold_lock,
};
use thiserror::Error;

use super::parent_loss_admission::{
    AdmissionIdentity, AdmissionResultState, ParentLossAdmissionError,
    ParentLossServiceWitnessDrop, admission_directory, read_parent_loss_admission_intent,
    read_parent_loss_admission_result, witness_path,
};
use super::parent_loss_ledger::{
    ActiveGeneration, BootstrapRecoveryReason, ParentLossGeneration, ParentLossLedger,
    ParentLossLedgerError, ParentLossPhase, ParentLossTerminalDisposition,
    ParentLossUnresolvedReason,
};
use super::{
    DeclaredParent, HostedServiceKind, ParentLossReason, ParentWatch, ParentWatchStatus,
    PlatformParentExitWatcher,
};
use crate::process::{
    InspectResult, ProcessInstance, ProcessInstanceSource, SystemProcessInstanceSource,
};

/// The parent-loss outcome must be resolved or durably refused no later than
/// this duration after a confirmed supervisor loss.
pub const PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE: Duration = Duration::from_secs(15);
const POLL_INTERVAL: Duration = Duration::from_millis(25);
const ADMISSION_SEAL_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
const BOOTSTRAP_SCHEMA: u32 = 1;
const FILE_MODE: u32 = 0o600;

/// Inputs supplied by the supervisor during its private bootstrap exchange.
#[derive(Clone, Debug)]
pub struct CoordinatorBootstrap {
    pub journal: PathBuf,
    pub supervisor: ProcessInstance,
    pub enabled: Vec<HostedServiceKind>,
    /// Random bytes received over the coordinator's private inherited stdin.
    pub capability: Vec<u8>,
}

/// Coordinator readiness persisted only after identity, record, and active
/// admission state have all been established.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoordinatorBootstrapReady {
    pub schema: u32,
    pub generation: ParentLossGeneration,
    pub coordinator: ProcessInstance,
    pub proof: String,
}

#[derive(Debug, Error)]
pub enum CoordinatorBootstrapError {
    #[error(transparent)]
    Ledger(#[from] ParentLossLedgerError),
    #[error("coordinator lease failed: {0}")]
    Lease(#[from] LeaseError),
    #[error("another coordinator currently holds the lifecycle lease")]
    Contended,
    #[error("coordinator could not identify itself")]
    SelfUnverifiable,
    #[error("coordinator readiness I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("coordinator readiness JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Debug, Error)]
pub enum ParentLossCoordinatorError {
    #[error(transparent)]
    Ledger(#[from] ParentLossLedgerError),
    #[error(transparent)]
    Admission(#[from] ParentLossAdmissionError),
    #[error("coordinator I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("coordinator JSON failed: {0}")]
    Json(#[from] serde_json::Error),
}

/// One owned coordinator.  `_lease` is intentionally retained until a clean
/// terminal release; unresolved state remains durably blocking even after the
/// operating system drops this process's advisory lock on exit.
pub struct ParentLossCoordinator {
    ledger: ParentLossLedger,
    _lease: Option<FileLease>,
    active: ActiveGeneration,
    coordinator: ProcessInstance,
    capability: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct SealedAdmission {
    launch_id: String,
    service: Option<HostedServiceKind>,
    identity: AdmissionIdentity,
}

/// A witness may establish that a service completed its own shutdown work, but
/// terminal success also requires an independent exact observation that every
/// sealed member has exited.
#[derive(Debug, Clone, PartialEq, Eq)]
enum RetirementValidation {
    Complete(ParentLossTerminalDisposition),
    Pending,
    Unresolved(ParentLossUnresolvedReason),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct RetireExpectedControl {
    schema: u32,
    generation: ParentLossGeneration,
    supervisor: ProcessInstance,
    proof: String,
}

impl ParentLossCoordinator {
    /// Acquire the process-lifetime lease and establish the generation's
    /// durable reservation, record, identity, and admission-ready pointer.
    pub fn bootstrap(
        bootstrap: CoordinatorBootstrap,
    ) -> Result<(Self, CoordinatorBootstrapReady), CoordinatorBootstrapError> {
        let ledger = ParentLossLedger::open(&bootstrap.journal)?;
        let Some(lease) =
            acquire_file_lease(ledger.coordinator_lease_path(), LeaseOptions::default())?
        else {
            return Err(CoordinatorBootstrapError::Contended);
        };
        let source = SystemProcessInstanceSource;
        let coordinator = match source.inspect(std::process::id()) {
            InspectResult::Present { instance, .. } => instance,
            InspectResult::Absent | InspectResult::Unverifiable => {
                return Err(CoordinatorBootstrapError::SelfUnverifiable);
            }
        };
        let active = ledger.reserve_generation(bootstrap.supervisor, bootstrap.enabled)?;
        // The following separate writes deliberately preserve the fault seams
        // after reservation confirmation.
        ledger.initialize_record(&active)?;
        let active = ledger.persist_coordinator_identity(active.generation, coordinator)?;
        let active = ledger.mark_admitting(active.generation, coordinator)?;
        let ready = CoordinatorBootstrapReady {
            schema: BOOTSTRAP_SCHEMA,
            generation: active.generation,
            coordinator,
            proof: bootstrap_proof(&bootstrap.capability, active.generation, coordinator),
        };
        write_bootstrap_ready(&ledger, &ready)?;
        Ok((
            Self {
                ledger,
                _lease: Some(lease),
                active,
                coordinator,
                capability: bootstrap.capability,
            },
            ready,
        ))
    }

    pub fn generation(&self) -> ParentLossGeneration {
        self.active.generation
    }

    pub fn coordinator_identity(&self) -> ProcessInstance {
        self.coordinator
    }

    pub fn read_bootstrap_ready(
        journal: &Path,
    ) -> Result<Option<CoordinatorBootstrapReady>, CoordinatorBootstrapError> {
        let ledger = ParentLossLedger::open(journal)?;
        match fs::read(bootstrap_path(&ledger)) {
            Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    pub fn bootstrap_ready_is_authenticated(
        ready: &CoordinatorBootstrapReady,
        capability: &[u8],
    ) -> bool {
        ready.schema == BOOTSTRAP_SCHEMA
            && ready.proof == bootstrap_proof(capability, ready.generation, ready.coordinator)
    }

    /// Run until normal retirement, parent loss, or a terminal fail-closed
    /// outcome.  The bounded wait is absolute from the first loss observation.
    pub fn run(mut self) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        let watch = ParentWatch::admit(
            DeclaredParent::from_instance(self.active.supervisor),
            &SystemProcessInstanceSource,
        );
        let watch = match watch {
            Ok(watch) => watch,
            Err(_) => {
                return self
                    .terminal_unresolved(ParentLossUnresolvedReason::ParentUnverifiable, None);
            }
        };
        let watcher = match PlatformParentExitWatcher::arm(watch.instance()) {
            Ok(watcher) => watcher,
            Err(_) => {
                return self
                    .terminal_unresolved(ParentLossUnresolvedReason::ParentUnverifiable, None);
            }
        };
        if let ParentWatchStatus::Lost(reason) = watch.check(&SystemProcessInstanceSource) {
            return match reason {
                ParentLossReason::ExitedOrReused => self.handle_confirmed_parent_loss(),
                ParentLossReason::Unverifiable => self.wait_unverifiable_parent(),
            };
        }
        let (loss_sender, loss_receiver) = std::sync::mpsc::channel();
        thread::Builder::new()
            .name("parent-loss-coordinator-watch".to_owned())
            .spawn(move || {
                let _ = loss_sender.send(watcher.wait_for_loss(watch));
            })
            .map_err(ParentLossCoordinatorError::Io)?;
        loop {
            if self.accept_retire_expected_if_live(&watch)? {
                let (sealed, digest) = self.seal_admissions()?;
                return self.wait_for_retirement(&sealed, digest, true);
            }
            match loss_receiver.try_recv() {
                Ok(ParentLossReason::ExitedOrReused) => {
                    return self.handle_confirmed_parent_loss();
                }
                Ok(ParentLossReason::Unverifiable)
                | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    return self.wait_unverifiable_parent();
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => thread::sleep(POLL_INTERVAL),
            }
        }
    }

    fn handle_confirmed_parent_loss(
        &mut self,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        let (sealed, digest) = self.seal_admissions()?;
        if sealed.is_empty() {
            let disposition = ParentLossTerminalDisposition::CancelledBeforeAdmission;
            self.publish_terminal(disposition.clone(), Some(digest))?;
            self.release_clean_lease();
            return Ok(disposition);
        }
        self.wait_for_retirement(&sealed, digest, false)
    }

    fn wait_for_retirement(
        &mut self,
        sealed: &[SealedAdmission],
        digest: String,
        expected_retirement: bool,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        self.wait_for_retirement_with_deadline(
            sealed,
            digest,
            expected_retirement,
            PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE,
        )
    }

    fn wait_for_retirement_with_deadline(
        &mut self,
        sealed: &[SealedAdmission],
        digest: String,
        expected_retirement: bool,
        timeout: Duration,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        self.wait_for_retirement_with_deadline_and_source(
            sealed,
            digest,
            expected_retirement,
            timeout,
            &SystemProcessInstanceSource,
        )
    }

    fn wait_for_retirement_with_deadline_and_source(
        &mut self,
        sealed: &[SealedAdmission],
        digest: String,
        expected_retirement: bool,
        timeout: Duration,
        source: &dyn ProcessInstanceSource,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        let deadline = Instant::now() + timeout;
        loop {
            match self.validate_witnesses_with_source(sealed, &digest, expected_retirement, source)
            {
                Ok(RetirementValidation::Complete(
                    ParentLossTerminalDisposition::Completed { .. }
                    | ParentLossTerminalDisposition::RetiredExpected,
                )) => {
                    let disposition = if expected_retirement {
                        ParentLossTerminalDisposition::RetiredExpected
                    } else {
                        ParentLossTerminalDisposition::Completed {
                            sealed_ledger_digest: digest.clone(),
                        }
                    };
                    self.publish_terminal(disposition.clone(), Some(digest))?;
                    self.release_clean_lease();
                    return Ok(disposition);
                }
                Ok(RetirementValidation::Pending) if Instant::now() < deadline => {
                    thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Ok(RetirementValidation::Pending) => {
                    return self.terminal_unresolved(
                        ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                            deadline_seconds: PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE.as_secs(),
                        },
                        Some(digest),
                    );
                }
                Ok(RetirementValidation::Unresolved(reason)) => {
                    return self.terminal_unresolved(reason, Some(digest));
                }
                Ok(RetirementValidation::Complete(_)) => {
                    return self.terminal_unresolved(
                        ParentLossUnresolvedReason::ArtifactFailure,
                        Some(digest),
                    );
                }
                Err(error) if Instant::now() < deadline => {
                    let _ = error;
                    thread::sleep(
                        POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())),
                    );
                }
                Err(_) => {
                    return self.terminal_unresolved(
                        ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                            deadline_seconds: PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE.as_secs(),
                        },
                        Some(digest),
                    );
                }
            }
        }
    }

    fn wait_unverifiable_parent(
        &mut self,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        self.wait_unverifiable_parent_with_deadline(PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE)
    }

    fn wait_unverifiable_parent_with_deadline(
        &mut self,
        timeout: Duration,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            thread::sleep(POLL_INTERVAL.min(deadline.saturating_duration_since(Instant::now())));
        }
        self.terminal_unresolved(
            ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                deadline_seconds: PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE.as_secs(),
            },
            None,
        )
    }

    fn seal_admissions(
        &mut self,
    ) -> Result<(Vec<SealedAdmission>, String), ParentLossCoordinatorError> {
        let _admission_lock = hold_lock(
            self.ledger.admission_lock_path(self.active.generation),
            LockOptions {
                timeout: ADMISSION_SEAL_LOCK_TIMEOUT,
                poll_interval: Duration::from_millis(10),
                mode: Some(FILE_MODE),
            },
        )
        .map_err(|error| ParentLossCoordinatorError::Ledger(ParentLossLedgerError::Lock(error)))?;
        self.active = self.ledger.seal(self.active.generation, self.coordinator)?;
        let directory = admission_directory(&self.ledger, self.active.generation);
        let mut entries = Vec::new();
        match fs::read_dir(&directory) {
            Ok(children) => {
                for child in children {
                    let child = child?;
                    if !child.file_type()?.is_dir() {
                        continue;
                    }
                    let launch_id = child.file_name().to_string_lossy().to_string();
                    let Some(intent) = read_parent_loss_admission_intent(
                        &self.ledger,
                        self.active.generation,
                        &launch_id,
                    )?
                    else {
                        return self.seal_failure(ParentLossUnresolvedReason::MissingAdmission);
                    };
                    let Some(result) = read_parent_loss_admission_result(
                        &self.ledger,
                        self.active.generation,
                        &launch_id,
                    )?
                    else {
                        return self.seal_failure(ParentLossUnresolvedReason::AdmissionUnconfirmed);
                    };
                    if intent.generation != self.active.generation
                        || result.identity.as_ref().is_some_and(|identity| {
                            identity.generation != intent.generation
                                || identity.launch_id != intent.launch_id
                        })
                    {
                        return self.seal_failure(ParentLossUnresolvedReason::AdmissionConflict);
                    }
                    match result.state {
                        AdmissionResultState::Admitted => {}
                        AdmissionResultState::RejectedUnreaped { .. } => {
                            return self
                                .seal_failure(ParentLossUnresolvedReason::AdmissionReapFailed);
                        }
                        AdmissionResultState::SpawnFailed { .. }
                        | AdmissionResultState::RejectedAndReaped { .. } => continue,
                    }
                    let Some(identity) = result.identity else {
                        return self.seal_failure(ParentLossUnresolvedReason::AdmissionUnconfirmed);
                    };
                    entries.push(SealedAdmission {
                        launch_id,
                        service: intent.service,
                        identity,
                    });
                }
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        entries.sort_by(|left, right| left.launch_id.cmp(&right.launch_id));
        let digest = self
            .ledger
            .write_sealed_ledger(self.active.generation, &entries)?;
        Ok((entries, digest))
    }

    fn seal_failure<T>(
        &mut self,
        reason: ParentLossUnresolvedReason,
    ) -> Result<T, ParentLossCoordinatorError> {
        let _ = self.terminal_unresolved(reason, None)?;
        Err(ParentLossCoordinatorError::Ledger(
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::RecordMalformed),
        ))
    }

    fn validate_witnesses_with_source(
        &self,
        sealed: &[SealedAdmission],
        digest: &str,
        expected_retirement: bool,
        source: &dyn ProcessInstanceSource,
    ) -> Result<RetirementValidation, ParentLossCoordinatorError> {
        let expected: BTreeSet<_> = self.active.enabled.iter().copied().collect();
        let admitted: BTreeSet<_> = sealed.iter().filter_map(|entry| entry.service).collect();
        if admitted != expected {
            return Ok(RetirementValidation::Unresolved(
                ParentLossUnresolvedReason::MissingAdmission,
            ));
        }
        for service in expected {
            let Some(admission) = sealed.iter().find(|entry| entry.service == Some(service)) else {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::MissingAdmission,
                ));
            };
            let Some(witness) = read_witness(&self.ledger, self.active.generation, service)? else {
                return Ok(RetirementValidation::Pending);
            };
            if witness.identity != admission.identity || witness.parent != self.active.supervisor {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::WitnessInvalid,
                ));
            }
            if !witness.listener_stopped {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::ListenerReleaseFailed,
                ));
            }
            if !witness.service_runner_stopped {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::ServiceRunnerDidNotStop,
                ));
            }
            if !witness.operational_artifacts_cleaned {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::OperationalArtifactsNotCleaned,
                ));
            }
            if let Some(failure) = witness.descendant_failure {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::Descendant(failure),
                ));
            }
            if !witness.descendants_retired || !witness.shutdown_complete {
                return Ok(RetirementValidation::Unresolved(
                    ParentLossUnresolvedReason::WitnessInvalid,
                ));
            }
        }
        for admission in sealed {
            match source.inspect(admission.identity.instance.pid) {
                InspectResult::Absent => continue,
                InspectResult::Unverifiable => return Ok(RetirementValidation::Pending),
                InspectResult::Present { instance, uid, .. }
                    if instance != admission.identity.instance || uid != admission.identity.uid =>
                {
                    return Ok(RetirementValidation::Unresolved(
                        ParentLossUnresolvedReason::AdmissionIdentityMismatch,
                    ));
                }
                InspectResult::Present { .. } => return Ok(RetirementValidation::Pending),
            }
        }
        if expected_retirement {
            Ok(RetirementValidation::Complete(
                ParentLossTerminalDisposition::RetiredExpected,
            ))
        } else {
            Ok(RetirementValidation::Complete(
                ParentLossTerminalDisposition::Completed {
                    sealed_ledger_digest: digest.to_owned(),
                },
            ))
        }
    }

    fn accept_retire_expected_if_live(
        &mut self,
        watch: &ParentWatch,
    ) -> Result<bool, ParentLossCoordinatorError> {
        let path = self
            .ledger
            .generation_path(self.active.generation)
            .join("control/retire-expected.json");
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(false),
            Err(error) => return Err(error.into()),
        };
        let control: RetireExpectedControl = serde_json::from_slice(&bytes)?;
        if control.schema != BOOTSTRAP_SCHEMA
            || control.generation != self.active.generation
            || control.supervisor != self.active.supervisor
            || control.proof
                != retire_proof(
                    &self.capability,
                    self.active.generation,
                    self.active.supervisor,
                )
        {
            return Ok(false);
        }
        if !matches!(
            watch.check(&SystemProcessInstanceSource),
            ParentWatchStatus::Live
        ) {
            return Ok(false);
        }
        // This write is the graceful-retirement linearization point. The
        // coordinator has authenticated the request and sampled the exact
        // supervisor as live before it handles a parent-loss observation.
        self.active = self
            .ledger
            .acknowledge_retirement(self.active.generation, self.coordinator)?;
        Ok(true)
    }

    fn publish_terminal(
        &mut self,
        disposition: ParentLossTerminalDisposition,
        digest: Option<String>,
    ) -> Result<(), ParentLossCoordinatorError> {
        self.ledger.write_terminal_with_digest(
            self.active.generation,
            self.coordinator,
            disposition,
            digest,
        )?;
        self.active.phase = ParentLossPhase::Terminal;
        Ok(())
    }

    fn terminal_unresolved(
        &mut self,
        reason: ParentLossUnresolvedReason,
        digest: Option<String>,
    ) -> Result<ParentLossTerminalDisposition, ParentLossCoordinatorError> {
        let digest = match digest {
            Some(digest) => digest,
            None => self
                .ledger
                .write_sealed_ledger(self.active.generation, &Vec::<SealedAdmission>::new())?,
        };
        let disposition = ParentLossTerminalDisposition::Unresolved { reason };
        self.publish_terminal(disposition.clone(), Some(digest))?;
        Ok(disposition)
    }

    fn release_clean_lease(&mut self) {
        self._lease.take();
    }
}

/// Write a one-time authenticated graceful-retirement instruction.  The
/// capability is held only in supervisor/coordinator memory, never in a
/// journal artifact.
pub fn write_retire_expected_control(
    journal: &Path,
    generation: ParentLossGeneration,
    supervisor: ProcessInstance,
    capability: &[u8],
) -> Result<(), ParentLossCoordinatorError> {
    let ledger = ParentLossLedger::open(journal)?;
    let path = ledger
        .generation_path(generation)
        .join("control/retire-expected.json");
    let control = RetireExpectedControl {
        schema: BOOTSTRAP_SCHEMA,
        generation,
        supervisor,
        proof: retire_proof(capability, generation, supervisor),
    };
    let parent = path.parent().expect("control parent");
    fs::create_dir_all(parent)?;
    let data = serde_json::to_vec_pretty(&control)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(&data)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn bootstrap_path(ledger: &ParentLossLedger) -> PathBuf {
    ledger.root_path().join("bootstrap-ready.json")
}

fn write_bootstrap_ready(
    ledger: &ParentLossLedger,
    ready: &CoordinatorBootstrapReady,
) -> Result<(), CoordinatorBootstrapError> {
    let path = bootstrap_path(ledger);
    let parent = path.parent().expect("bootstrap parent");
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(ready)?;
    let mut options = fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(FILE_MODE);
    }
    let mut file = options.open(path)?;
    use std::io::Write;
    file.write_all(&bytes)?;
    file.write_all(b"\n")?;
    file.sync_all()?;
    Ok(())
}

fn read_witness(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    service: HostedServiceKind,
) -> Result<Option<ParentLossServiceWitnessDrop>, ParentLossCoordinatorError> {
    let path = witness_path(ledger, generation, service);
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn bootstrap_proof(
    capability: &[u8],
    generation: ParentLossGeneration,
    coordinator: ProcessInstance,
) -> String {
    proof(capability, b"bootstrap", generation, coordinator)
}

fn retire_proof(
    capability: &[u8],
    generation: ParentLossGeneration,
    supervisor: ProcessInstance,
) -> String {
    proof(capability, b"retire", generation, supervisor)
}

fn proof(
    capability: &[u8],
    domain: &[u8],
    generation: ParentLossGeneration,
    identity: ProcessInstance,
) -> String {
    let mut hasher = Sha256::new();
    hasher.update(capability);
    hasher.update(domain);
    hasher.update(generation.to_le_bytes());
    hasher.update(serde_json::to_vec(&identity).expect("process identity serializes"));
    format!("{:x}", hasher.finalize())
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;
    use std::sync::Mutex;

    use tempfile::TempDir;

    use super::*;
    use crate::lifecycle::{
        AdmissionIntent, AdmissionResult, ParentLossReaderOutcome, ParentLossServiceWitnessDrop,
        read_parent_loss_outcome, write_parent_loss_admission_intent,
        write_parent_loss_admission_result, write_parent_loss_service_witness,
    };
    use crate::process::{
        Disposition, HostedAdmissionTestFault, HostedLaunchProvenance, InstanceCensus,
        ManagedLaunchRequest, ProcessBirth, SpawnOptions, launch_managed_hosted,
        set_hosted_admission_test_fault,
    };

    struct MutableInspectSource {
        result: Mutex<InspectResult>,
    }

    impl ProcessInstanceSource for MutableInspectSource {
        fn inspect(&self, _pid: u32) -> InspectResult {
            *self.result.lock().expect("inspect result lock")
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Complete(Vec::new())
        }
    }

    fn gone_process_source() -> MutableInspectSource {
        MutableInspectSource {
            result: Mutex::new(InspectResult::Absent),
        }
    }

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn bootstrap(
        enabled: impl IntoIterator<Item = HostedServiceKind>,
    ) -> (TempDir, ParentLossCoordinator, DeclaredParent, Vec<u8>) {
        let journal = TempDir::new().expect("temporary journal");
        let declared_parent = DeclaredParent::capture_current().expect("live direct parent");
        let capability = b"coordinator-test-capability".to_vec();
        let (coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: journal.path().to_path_buf(),
            supervisor: declared_parent.instance(),
            enabled: enabled.into_iter().collect(),
            capability: capability.clone(),
        })
        .expect("coordinator bootstrap");
        (journal, coordinator, declared_parent, capability)
    }

    fn admitted_identity(
        coordinator: &ParentLossCoordinator,
        service: HostedServiceKind,
    ) -> AdmissionIdentity {
        AdmissionIdentity {
            generation: coordinator.generation(),
            launch_id: format!("{service:?}").to_lowercase(),
            instance: instance(100 + service as u32, 10 + service as u64),
            uid: 501,
            parent_launch_id: None,
        }
    }

    fn write_admitted_service(
        journal: &Path,
        coordinator: &ParentLossCoordinator,
        service: HostedServiceKind,
    ) -> AdmissionIdentity {
        let identity = admitted_identity(coordinator, service);
        write_parent_loss_admission_intent(
            journal,
            &AdmissionIntent::new(
                coordinator.generation(),
                identity.launch_id.clone(),
                Some(service),
                None,
            ),
        )
        .expect("admission intent");
        write_parent_loss_admission_result(
            journal,
            coordinator.generation(),
            &identity.launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity.clone()),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("admission result");
        write_parent_loss_service_witness(
            journal,
            &ParentLossServiceWitnessDrop {
                schema: 1,
                service,
                parent: coordinator.active.supervisor,
                identity: identity.clone(),
                listener_stopped: true,
                service_runner_stopped: true,
                operational_artifacts_cleaned: true,
                descendants_retired: true,
                shutdown_complete: true,
                descendant_failure: None,
            },
        )
        .expect("service witness");
        identity
    }

    fn terminal_unresolved_after_witness_failure(
        journal: &Path,
        coordinator: &mut ParentLossCoordinator,
        service: HostedServiceKind,
        mutate: impl FnOnce(&mut ParentLossServiceWitnessDrop),
        expected: ParentLossUnresolvedReason,
    ) {
        let identity = admitted_identity(coordinator, service);
        write_parent_loss_admission_intent(
            journal,
            &AdmissionIntent::new(
                coordinator.generation(),
                identity.launch_id.clone(),
                Some(service),
                None,
            ),
        )
        .expect("admission intent");
        write_parent_loss_admission_result(
            journal,
            coordinator.generation(),
            &identity.launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity.clone()),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("admission result");
        let mut witness = ParentLossServiceWitnessDrop {
            schema: 1,
            service,
            parent: coordinator.active.supervisor,
            identity,
            listener_stopped: true,
            service_runner_stopped: true,
            operational_artifacts_cleaned: true,
            descendants_retired: true,
            shutdown_complete: true,
            descendant_failure: None,
        };
        mutate(&mut witness);
        write_parent_loss_service_witness(journal, &witness).expect("service witness");
        let (sealed, digest) = coordinator.seal_admissions().expect("sealed admissions");
        let source = gone_process_source();
        assert_eq!(
            coordinator
                .validate_witnesses_with_source(&sealed, &digest, false, &source)
                .expect("witness validation"),
            RetirementValidation::Unresolved(expected.clone())
        );
        assert_eq!(
            coordinator
                .terminal_unresolved(expected.clone(), Some(digest))
                .expect("durable unresolved terminal"),
            ParentLossTerminalDisposition::Unresolved { reason: expected }
        );
        assert!(coordinator._lease.is_some());
    }

    #[test]
    fn authenticated_live_retirement_is_acknowledged_before_terminal_retirement() {
        let journal = TempDir::new().expect("temporary journal");
        let declared_parent = DeclaredParent::capture_current().expect("live direct parent");
        let supervisor = declared_parent.instance();
        let capability = b"coordinator-retirement-test-capability".to_vec();
        let (mut coordinator, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: journal.path().to_path_buf(),
            supervisor,
            enabled: Vec::new(),
            capability: capability.clone(),
        })
        .expect("coordinator bootstrap");
        write_retire_expected_control(
            journal.path(),
            coordinator.generation(),
            supervisor,
            &capability,
        )
        .expect("authenticated retirement request");
        let watch = ParentWatch::admit(declared_parent, &SystemProcessInstanceSource)
            .expect("supervisor remains live while request is accepted");

        assert!(
            coordinator
                .accept_retire_expected_if_live(&watch)
                .expect("accept authenticated live retirement")
        );
        assert_eq!(
            coordinator
                .ledger
                .active_generation()
                .expect("read active generation")
                .expect("active generation")
                .phase,
            ParentLossPhase::RetiringAcknowledged
        );

        let (sealed, digest) = coordinator.seal_admissions().expect("seal empty roster");
        assert_eq!(
            coordinator
                .wait_for_retirement(&sealed, digest, true)
                .expect("retire expected generation"),
            ParentLossTerminalDisposition::RetiredExpected
        );
    }

    #[test]
    fn same_root_bootstrap_has_exactly_one_coordinator_authority() {
        let (journal, coordinator, declared_parent, capability) = bootstrap([]);
        let result = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: journal.path().to_path_buf(),
            supervisor: declared_parent.instance(),
            enabled: Vec::new(),
            capability,
        });
        assert!(matches!(result, Err(CoordinatorBootstrapError::Contended)));
        assert!(
            ParentLossCoordinator::read_bootstrap_ready(journal.path())
                .expect("bootstrap readiness")
                .is_some()
        );
        assert_eq!(
            coordinator
                .ledger
                .active_generation()
                .expect("active")
                .expect("active generation")
                .generation,
            coordinator.generation()
        );
    }

    #[test]
    fn coordinator_bootstrap_admits_the_first_service_under_one_generation() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        let identity =
            write_admitted_service(journal.path(), &coordinator, HostedServiceKind::Sense);
        let (sealed, _) = coordinator.seal_admissions().expect("seal admissions");
        assert_eq!(sealed.len(), 1);
        assert_eq!(sealed[0].identity, identity);
        assert_eq!(sealed[0].service, Some(HostedServiceKind::Sense));
    }

    #[test]
    fn distinct_roots_bootstrap_independently() {
        let (first_journal, first, _, _) = bootstrap([]);
        let (second_journal, second, _, _) = bootstrap([]);
        assert_ne!(first_journal.path(), second_journal.path());
        assert_eq!(first.generation(), 1);
        assert_eq!(second.generation(), 1);
        assert_ne!(
            first.ledger.canonical_root(),
            second.ledger.canonical_root()
        );
    }

    #[test]
    fn missing_admission_result_becomes_durable_unresolved_without_lease_release() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        let intent = AdmissionIntent::new(
            coordinator.generation(),
            "sense",
            Some(HostedServiceKind::Sense),
            None,
        );
        write_parent_loss_admission_intent(journal.path(), &intent).expect("intent");
        assert!(coordinator.seal_admissions().is_err());
        let record = coordinator
            .ledger
            .record(coordinator.generation())
            .expect("record")
            .expect("record exists");
        assert_eq!(
            record.terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AdmissionUnconfirmed,
            })
        );
        assert!(coordinator._lease.is_some());
    }

    #[test]
    fn loss_before_any_admission_cancels_and_permits_one_clean_successor() {
        let (journal, mut coordinator, _, _) = bootstrap([]);
        assert_eq!(
            coordinator
                .handle_confirmed_parent_loss()
                .expect("cancel before admission"),
            ParentLossTerminalDisposition::CancelledBeforeAdmission
        );
        assert!(coordinator._lease.is_none());
        let successor = ParentLossLedger::open(journal.path())
            .expect("ledger")
            .reserve_generation(instance(200, 20), [])
            .expect("clean successor");
        assert_eq!(successor.generation, 2);
    }

    #[test]
    fn forged_retire_instruction_without_private_capability_is_rejected() {
        let (journal, mut coordinator, declared_parent, _) = bootstrap([]);
        write_retire_expected_control(
            journal.path(),
            coordinator.generation(),
            declared_parent.instance(),
            b"same-uid-peer-does-not-have-capability",
        )
        .expect("forged control record");
        let watch = ParentWatch::admit(declared_parent, &SystemProcessInstanceSource)
            .expect("supervisor stays live");
        assert!(
            !coordinator
                .accept_retire_expected_if_live(&watch)
                .expect("reject forged request")
        );
        assert_eq!(coordinator.active.phase, ParentLossPhase::Admitting);
        assert!(coordinator._lease.is_some());
    }

    #[test]
    fn early_stale_and_replayed_retire_instructions_do_not_authorize_a_generation() {
        let (journal, mut coordinator, declared_parent, capability) = bootstrap([]);
        write_retire_expected_control(
            journal.path(),
            coordinator.generation() + 1,
            declared_parent.instance(),
            &capability,
        )
        .expect("early next-generation control");
        let watch = ParentWatch::admit(declared_parent, &SystemProcessInstanceSource)
            .expect("supervisor stays live");
        assert!(
            !coordinator
                .accept_retire_expected_if_live(&watch)
                .expect("ignore early control")
        );
        assert!(coordinator._lease.is_some());

        write_retire_expected_control(
            journal.path(),
            coordinator.generation(),
            declared_parent.instance(),
            &capability,
        )
        .expect("current control");
        assert!(matches!(
            write_retire_expected_control(
                journal.path(),
                coordinator.generation(),
                declared_parent.instance(),
                &capability,
            ),
            Err(ParentLossCoordinatorError::Io(_))
        ));
        assert!(
            coordinator
                .accept_retire_expected_if_live(&watch)
                .expect("accept current control")
        );
        assert!(
            coordinator.accept_retire_expected_if_live(&watch).is_err(),
            "replay cannot re-linearize retirement"
        );
    }

    #[test]
    fn parent_loss_wins_when_sealing_precedes_retirement_acknowledgement() {
        let (_journal, coordinator, _, _) = bootstrap([]);
        coordinator
            .ledger
            .seal(coordinator.generation(), coordinator.coordinator)
            .expect("parent-loss seal wins");
        assert!(matches!(
            coordinator
                .ledger
                .acknowledge_retirement(coordinator.generation(), coordinator.coordinator),
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::ReservationIncomplete
            ))
        ));
        assert!(coordinator._lease.is_some());
    }

    #[test]
    fn valid_witnesses_produce_completed_and_release_clean_lease() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        write_admitted_service(journal.path(), &coordinator, HostedServiceKind::Sense);
        let (sealed, digest) = coordinator.seal_admissions().expect("sealed admissions");
        let source = gone_process_source();
        assert_eq!(
            coordinator
                .wait_for_retirement_with_deadline_and_source(
                    &sealed,
                    digest,
                    false,
                    Duration::from_millis(1),
                    &source,
                )
                .expect("completed terminal"),
            ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest: coordinator
                    .ledger
                    .record(coordinator.generation())
                    .expect("record")
                    .expect("record exists")
                    .sealed_ledger_digest
                    .expect("digest"),
            }
        );
        assert!(coordinator._lease.is_none());
    }

    #[test]
    fn successful_witness_waits_for_independent_exact_service_retirement() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        let identity =
            write_admitted_service(journal.path(), &coordinator, HostedServiceKind::Sense);
        let (sealed, digest) = coordinator.seal_admissions().expect("sealed admissions");
        let source = MutableInspectSource {
            result: Mutex::new(InspectResult::Present {
                instance: identity.instance,
                uid: identity.uid,
                execution: crate::process::ExecutionState::Running,
                ppid: None,
                pgid: None,
            }),
        };

        assert_eq!(
            coordinator
                .validate_witnesses_with_source(&sealed, &digest, false, &source)
                .expect("live service observation"),
            RetirementValidation::Pending,
            "a self-reported witness cannot complete a still-live service"
        );
        assert!(
            coordinator
                .ledger
                .record(coordinator.generation())
                .expect("record")
                .expect("record exists")
                .terminal
                .is_none(),
            "the pending live observation must not write a completed terminal record"
        );

        *source.result.lock().expect("inspect result lock") = InspectResult::Absent;
        assert!(matches!(
            coordinator
                .validate_witnesses_with_source(&sealed, &digest, false, &source)
                .expect("retired service observation"),
            RetirementValidation::Complete(ParentLossTerminalDisposition::Completed { .. })
        ));
    }

    #[test]
    fn authenticated_expected_retirement_validates_witnesses_and_permits_successor() {
        let (journal, mut coordinator, declared_parent, capability) =
            bootstrap([HostedServiceKind::Sense]);
        write_admitted_service(journal.path(), &coordinator, HostedServiceKind::Sense);
        write_retire_expected_control(
            journal.path(),
            coordinator.generation(),
            declared_parent.instance(),
            &capability,
        )
        .expect("authenticated control");
        let watch = ParentWatch::admit(declared_parent, &SystemProcessInstanceSource)
            .expect("supervisor remains live");
        assert!(
            coordinator
                .accept_retire_expected_if_live(&watch)
                .expect("retirement acknowledgement")
        );
        let (sealed, digest) = coordinator.seal_admissions().expect("sealed admissions");
        let source = gone_process_source();
        assert_eq!(
            coordinator
                .wait_for_retirement_with_deadline_and_source(
                    &sealed,
                    digest,
                    true,
                    Duration::from_millis(1),
                    &source,
                )
                .expect("expected retirement terminal"),
            ParentLossTerminalDisposition::RetiredExpected
        );
        assert!(coordinator._lease.is_none());
        assert_eq!(
            ParentLossLedger::open(journal.path())
                .expect("ledger")
                .reserve_generation(instance(300, 30), [])
                .expect("successor")
                .generation,
            2
        );
    }

    #[test]
    fn stale_retire_instruction_for_a_completed_generation_is_ignored() {
        let (journal, mut first, declared_parent, capability) = bootstrap([]);
        let old_generation = first.generation();
        first
            .handle_confirmed_parent_loss()
            .expect("cancel first generation");
        drop(first);
        write_retire_expected_control(
            journal.path(),
            old_generation,
            declared_parent.instance(),
            &capability,
        )
        .expect("stale control");
        let (mut successor, _) = ParentLossCoordinator::bootstrap(CoordinatorBootstrap {
            journal: journal.path().to_path_buf(),
            supervisor: declared_parent.instance(),
            enabled: Vec::new(),
            capability,
        })
        .expect("successor coordinator");
        assert_eq!(successor.generation(), old_generation + 1);
        let watch = ParentWatch::admit(declared_parent, &SystemProcessInstanceSource)
            .expect("supervisor remains live");
        assert!(
            !successor
                .accept_retire_expected_if_live(&watch)
                .expect("ignore stale control")
        );
        assert!(successor._lease.is_some());
    }

    #[test]
    fn descendant_retirement_failure_is_durable_unresolved_and_blocks_successor() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        terminal_unresolved_after_witness_failure(
            journal.path(),
            &mut coordinator,
            HostedServiceKind::Sense,
            |witness| {
                witness.descendants_retired = false;
                witness.descendant_failure =
                    Some(crate::process::DescendantObservationFailure::Reused);
            },
            ParentLossUnresolvedReason::Descendant(
                crate::process::DescendantObservationFailure::Reused,
            ),
        );
        assert!(
            coordinator
                .ledger
                .reserve_generation(instance(400, 40), [])
                .is_err()
        );
    }

    #[test]
    fn listener_release_failure_is_durable_unresolved_and_never_completed() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        terminal_unresolved_after_witness_failure(
            journal.path(),
            &mut coordinator,
            HostedServiceKind::Sense,
            |witness| witness.listener_stopped = false,
            ParentLossUnresolvedReason::ListenerReleaseFailed,
        );
        assert!(!matches!(
            coordinator
                .ledger
                .record(coordinator.generation())
                .expect("record")
                .expect("record exists")
                .terminal,
            Some(ParentLossTerminalDisposition::Completed { .. })
        ));
    }

    #[test]
    fn wrong_birth_witness_is_durable_unresolved_and_retains_lease() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        terminal_unresolved_after_witness_failure(
            journal.path(),
            &mut coordinator,
            HostedServiceKind::Sense,
            |witness| witness.identity.instance = instance(101, 999),
            ParentLossUnresolvedReason::WitnessInvalid,
        );
    }

    #[test]
    fn unverifiable_parent_deadline_uses_typed_unresolved_and_exits_without_successor() {
        let (journal, mut coordinator, _, _) = bootstrap([]);
        assert_eq!(
            coordinator
                .wait_unverifiable_parent_with_deadline(Duration::ZERO)
                .expect("deadline terminal"),
            ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                    deadline_seconds: PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE.as_secs(),
                },
            }
        );
        assert!(coordinator._lease.is_some());
        assert!(
            ParentLossLedger::open(journal.path())
                .expect("fresh reader ledger")
                .reserve_generation(instance(500, 50), [])
                .is_err()
        );
    }

    #[test]
    fn failed_terminal_persistence_leaves_the_confirmed_nonterminal_generation_blocking() {
        let (journal, mut coordinator, _, _) = bootstrap([]);
        std::fs::remove_file(coordinator.ledger.record_path(coordinator.generation()))
            .expect("remove record before terminal write");
        assert!(
            coordinator
                .terminal_unresolved(ParentLossUnresolvedReason::ArtifactFailure, None)
                .is_err()
        );
        assert!(matches!(
            read_parent_loss_outcome(journal.path()).expect("fresh reader outcome"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired { .. }
        ));
        assert!(coordinator._lease.is_some());
        assert!(
            coordinator
                .ledger
                .reserve_generation(instance(600, 60), [])
                .is_err()
        );
    }

    #[test]
    fn interrupted_record_state_is_not_misread_as_a_completed_generation() {
        let (journal, coordinator, _, _) = bootstrap([]);
        std::fs::write(
            coordinator.ledger.record_path(coordinator.generation()),
            b"{partial-json",
        )
        .expect("partial record fixture");
        drop(coordinator);
        assert_eq!(
            read_parent_loss_outcome(journal.path()).expect("fresh reader outcome"),
            ParentLossReaderOutcome::MalformedState {
                artifact: "record.json".to_owned(),
            }
        );
        assert!(
            ParentLossLedger::open(journal.path())
                .expect("fresh ledger")
                .reserve_generation(instance(700, 70), [])
                .is_err()
        );
    }

    #[test]
    fn seal_waits_for_the_admission_boundary_and_includes_the_preseal_result() {
        let (journal, coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        let generation = coordinator.generation();
        let supervisor = coordinator.active.supervisor;
        let identity = admitted_identity(&coordinator, HostedServiceKind::Sense);
        let lock = hold_lock(
            coordinator.ledger.admission_lock_path(generation),
            LockOptions {
                timeout: Duration::from_secs(1),
                poll_interval: Duration::from_millis(5),
                mode: Some(FILE_MODE),
            },
        )
        .expect("launch-boundary lock");
        let sealing = std::thread::spawn(move || {
            let mut coordinator = coordinator;
            let result = coordinator.seal_admissions();
            (coordinator, result)
        });
        std::thread::sleep(Duration::from_millis(20));
        write_parent_loss_admission_intent(
            journal.path(),
            &AdmissionIntent::new(
                generation,
                identity.launch_id.clone(),
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("intent inside launch boundary");
        write_parent_loss_admission_result(
            journal.path(),
            generation,
            &identity.launch_id,
            &AdmissionResult {
                schema: 1,
                identity: Some(identity.clone()),
                state: AdmissionResultState::Admitted,
            },
        )
        .expect("result inside launch boundary");
        write_parent_loss_service_witness(
            journal.path(),
            &ParentLossServiceWitnessDrop {
                schema: 1,
                service: HostedServiceKind::Sense,
                parent: supervisor,
                identity: identity.clone(),
                listener_stopped: true,
                service_runner_stopped: true,
                operational_artifacts_cleaned: true,
                descendants_retired: true,
                shutdown_complete: true,
                descendant_failure: None,
            },
        )
        .expect("witness inside launch boundary");
        drop(lock);
        let (_, sealed_result) = sealing.join().expect("seal thread");
        let (sealed, _) = sealed_result.expect("seal after admission boundary");
        assert_eq!(sealed.len(), 1);
        assert_eq!(sealed[0].identity, identity);
    }

    #[test]
    fn fresh_reader_validates_completed_record_before_successor_allocation() {
        let (journal, mut coordinator, _, _) = bootstrap([HostedServiceKind::Sense]);
        write_admitted_service(journal.path(), &coordinator, HostedServiceKind::Sense);
        let (sealed, digest) = coordinator.seal_admissions().expect("sealed admissions");
        let source = gone_process_source();
        let terminal = coordinator
            .wait_for_retirement_with_deadline_and_source(
                &sealed,
                digest,
                false,
                Duration::from_millis(1),
                &source,
            )
            .expect("completed terminal");
        assert!(matches!(
            terminal,
            ParentLossTerminalDisposition::Completed { .. }
        ));
        let fresh = read_parent_loss_outcome(journal.path()).expect("fresh reader outcome");
        assert!(matches!(fresh, ParentLossReaderOutcome::Completed { .. }));
        let successor = ParentLossLedger::open(journal.path())
            .expect("fresh ledger")
            .reserve_generation(instance(900, 90), [])
            .expect("successor after fresh validation");
        assert_eq!(successor.generation, 2);
    }

    #[cfg(unix)]
    fn reject_child_without_exact_reap(
        journal: &Path,
        coordinator: &ParentLossCoordinator,
        launch_id: &str,
        fault: HostedAdmissionTestFault,
    ) {
        set_hosted_admission_test_fault(Some(fault));
        let result = launch_managed_hosted(
            Disposition::InheritedParentScope,
            ManagedLaunchRequest {
                command: vec!["/bin/sleep".to_owned(), "60".to_owned()],
                options: SpawnOptions {
                    journal_root: journal.to_path_buf(),
                    reference: launch_id.to_owned(),
                    day: None,
                    sink: None,
                    environment: BTreeMap::new(),
                },
            },
            HostedLaunchProvenance {
                journal: journal.to_path_buf(),
                generation: coordinator.generation(),
                launch_id: launch_id.to_owned(),
                service: None,
                parent_launch_id: None,
                acknowledgement_timeout: Duration::from_millis(20),
            },
        );
        set_hosted_admission_test_fault(None);
        assert!(result.is_err(), "unacknowledged child must be rejected");
    }

    #[cfg(unix)]
    #[test]
    fn exact_reap_failure_is_durable_unresolved_and_blocks_successor() {
        let (journal, mut coordinator, _, _) = bootstrap([]);
        reject_child_without_exact_reap(
            journal.path(),
            &coordinator,
            "exact-reap-failure",
            HostedAdmissionTestFault::ExactReap,
        );

        assert!(coordinator.seal_admissions().is_err());
        assert_eq!(
            coordinator
                .ledger
                .record(coordinator.generation())
                .expect("record")
                .expect("record exists")
                .terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AdmissionReapFailed,
            })
        );
        assert!(coordinator._lease.is_some());
        assert!(
            coordinator
                .ledger
                .reserve_generation(instance(902, 92), [])
                .is_err()
        );
    }

    #[cfg(unix)]
    #[test]
    fn exact_exit_proof_failure_is_durable_unresolved_and_blocks_successor() {
        let (journal, mut coordinator, _, _) = bootstrap([]);
        reject_child_without_exact_reap(
            journal.path(),
            &coordinator,
            "exit-proof-failure",
            HostedAdmissionTestFault::ExitProof,
        );

        assert!(coordinator.seal_admissions().is_err());
        assert_eq!(
            coordinator
                .ledger
                .record(coordinator.generation())
                .expect("record")
                .expect("record exists")
                .terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AdmissionReapFailed,
            })
        );
        assert!(coordinator._lease.is_some());
    }

    #[cfg(unix)]
    #[test]
    fn exact_reap_failure_with_terminal_write_error_retains_nonterminal_lease() {
        let (journal, mut coordinator, _, _) = bootstrap([]);
        reject_child_without_exact_reap(
            journal.path(),
            &coordinator,
            "terminal-write-failure",
            HostedAdmissionTestFault::ExactReap,
        );
        std::fs::remove_file(coordinator.ledger.record_path(coordinator.generation()))
            .expect("force terminal record persistence failure");

        assert!(coordinator.seal_admissions().is_err());
        assert!(matches!(
            read_parent_loss_outcome(journal.path()).expect("fresh reader outcome"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired { .. }
        ));
        assert!(coordinator._lease.is_some());
        assert!(
            coordinator
                .ledger
                .reserve_generation(instance(903, 93), [])
                .is_err()
        );
    }
}
