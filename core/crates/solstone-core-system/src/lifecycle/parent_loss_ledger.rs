// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Canonical, generation-addressed parent-loss lifecycle records.
//!
//! This module owns the durable state shape and its short critical sections.
//! It deliberately does not adjudicate service evidence: that is the
//! coordinator's job.  In particular, no hosted service is allowed to mutate
//! a generation record or its sealed ledger.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use solstone_core_journal_io::{
    AtomicWriteError, JournalRoot, JournalRootError, JsonWriteOptions, LockError, LockOptions,
    hold_lock, write_json,
};
use thiserror::Error;

use crate::process::{
    DescendantObservationFailure, InstanceVerdict, ProcessInstance, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

pub const PARENT_LOSS_LEDGER_SCHEMA_V1: u32 = 1;
const FILE_MODE: u32 = 0o600;
const LOCK_TIMEOUT: Duration = Duration::from_secs(2);

/// Monotonically allocated lifecycle generation for one canonical journal.
pub type ParentLossGeneration = u64;

/// The fixed set of services hosted by the Journal supervisor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HostedServiceKind {
    Convey,
    Sense,
    Cortex,
    Spl,
}

/// Durable state of an active generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentLossPhase {
    Allocated,
    Admitting,
    /// The coordinator authenticated a graceful-retirement request while the
    /// exact supervisor was still live.  This durable acknowledgement is held
    /// until the generation reaches its terminal retirement disposition.
    RetiringAcknowledged,
    Sealed,
    Terminal,
}

/// Reservation milestones intentionally distinguish a clean pre-reservation
/// retry from a confirmed lifecycle domain which must never self-release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BootstrapReservation {
    /// The active pointer has been durably reserved; subsequent failures block.
    Confirmed,
    /// The coordinator identity was persisted into the active pointer.
    CoordinatorIdentityPersisted,
    /// The authenticated initial handshake completed and launches may admit.
    InitialAdmissionPersisted,
}

/// The pointer guarded by the short-lived active-generation lock.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ActiveGeneration {
    pub schema: u32,
    pub generation: ParentLossGeneration,
    pub coordinator: Option<ProcessInstance>,
    pub supervisor: ProcessInstance,
    pub enabled: Vec<HostedServiceKind>,
    pub phase: ParentLossPhase,
    pub reservation: BootstrapReservation,
}

/// The only terminal choices the coordinator can publish.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentLossTerminalDisposition {
    Completed { sealed_ledger_digest: String },
    Unresolved { reason: ParentLossUnresolvedReason },
    RetiredExpected,
    CancelledBeforeAdmission,
}

/// A typed reason a coordinator must fail closed.
#[derive(Debug, Clone, PartialEq, Eq, Error, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ParentLossUnresolvedReason {
    #[error("parent observation was unverifiable")]
    ParentUnverifiable,
    #[error("an admission drop is missing")]
    MissingAdmission,
    #[error("an admission did not receive a child acknowledgement")]
    AdmissionUnconfirmed,
    #[error("admission drops conflict")]
    AdmissionConflict,
    #[error("admission identity did not match the admitted child")]
    AdmissionIdentityMismatch,
    #[error("a seal-rejected child could not be exactly reaped")]
    AdmissionReapFailed,
    #[error("a required service witness is missing")]
    MissingWitness,
    #[error("service witnesses conflict")]
    WitnessConflict,
    #[error("service witness was invalid")]
    WitnessInvalid,
    #[error("owned listener did not release")]
    ListenerReleaseFailed,
    #[error("service runner did not stop")]
    ServiceRunnerDidNotStop,
    #[error("operational artifacts were not cleaned")]
    OperationalArtifactsNotCleaned,
    #[error("descendant observation failed: {0}")]
    Descendant(DescendantObservationFailure),
    #[error("retirement deadline of {deadline_seconds}s elapsed")]
    RetirementDeadlineExceeded { deadline_seconds: u64 },
    #[error("generation did not match")]
    GenerationMismatch,
    #[error("lifecycle record was malformed")]
    MalformedRecord,
    #[error("lifecycle artifact operation failed")]
    ArtifactFailure,
}

/// A nonterminal record written at reservation, followed by exactly one
/// coordinator-authorized terminal transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentLossGenerationRecord {
    pub schema: u32,
    pub generation: ParentLossGeneration,
    pub coordinator: Option<ProcessInstance>,
    pub supervisor: ProcessInstance,
    /// Digest of the immutable sealed ledger for every outcome reached after
    /// sealing, including unresolved and graceful retirement.
    pub sealed_ledger_digest: Option<String>,
    pub terminal: Option<ParentLossTerminalDisposition>,
}

/// Reader/start outcome.  Bootstrap recovery is intentionally not terminal:
/// it communicates that a durable nonterminal generation still owns the root.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ParentLossReaderOutcome {
    CleanFirstUse,
    Active {
        generation: ParentLossGeneration,
        phase: ParentLossPhase,
    },
    Completed {
        generation: ParentLossGeneration,
        sealed_ledger_digest: String,
    },
    Unresolved {
        generation: ParentLossGeneration,
        reason: ParentLossUnresolvedReason,
    },
    RetiredExpected {
        generation: ParentLossGeneration,
    },
    CancelledBeforeAdmission {
        generation: ParentLossGeneration,
    },
    BootstrapRecoveryRequired {
        generation: Option<ParentLossGeneration>,
        reason: BootstrapRecoveryReason,
    },
    MissingState {
        artifact: String,
    },
    InaccessibleState {
        artifact: String,
    },
    MalformedState {
        artifact: String,
    },
    WrongGeneration {
        expected: ParentLossGeneration,
        actual: ParentLossGeneration,
    },
    ConflictingResult {
        artifact: String,
    },
    PartialWitnessSet {
        generation: ParentLossGeneration,
        expected: Vec<HostedServiceKind>,
        found: Vec<HostedServiceKind>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BootstrapRecoveryReason {
    ActiveCoordinator,
    CoordinatorMissing,
    CoordinatorNotLive,
    ReservationIncomplete,
    RecordMissing,
    RecordMalformed,
    RecordWriteFailed,
}

/// Errors for the lifecycle domain's durable primitives.
#[derive(Debug, Error)]
pub enum ParentLossLedgerError {
    #[error("could not open canonical journal root: {0}")]
    JournalRoot(#[from] JournalRootError),
    #[error("parent-loss ledger I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("parent-loss ledger lock failed: {0}")]
    Lock(#[from] LockError),
    #[error("parent-loss ledger write failed: {0}")]
    Write(#[from] AtomicWriteError),
    #[error("parent-loss ledger JSON is malformed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("generation {generation} is not the active generation")]
    WrongGeneration { generation: ParentLossGeneration },
    #[error("generation record is already terminal")]
    AlreadyTerminal,
    #[error("terminal result does not match the active coordinator")]
    StaleCoordinator,
    #[error("active lifecycle generation prevents allocation: {0:?}")]
    RecoveryRequired(BootstrapRecoveryReason),
}

/// Canonical path authority and paths for one Journal lifecycle domain.
///
/// Retaining `JournalRoot` makes the initial canonicalization a capability
/// acquisition rather than path normalization performed by callers.
pub struct ParentLossLedger {
    _root: JournalRoot,
    canonical_root: PathBuf,
}

impl ParentLossLedger {
    pub fn open(journal: &Path) -> Result<Self, ParentLossLedgerError> {
        let root = JournalRoot::open(journal)?;
        let canonical_root = root.canonical_path().to_path_buf();
        Ok(Self {
            _root: root,
            canonical_root,
        })
    }

    pub fn canonical_root(&self) -> &Path {
        &self.canonical_root
    }

    pub fn root_path(&self) -> PathBuf {
        self.canonical_root.join("health/parent-loss")
    }

    pub fn coordinator_lease_path(&self) -> PathBuf {
        self.root_path().join("coordinator.lease")
    }

    pub fn active_path(&self) -> PathBuf {
        self.root_path().join("active-generation.json")
    }

    pub fn admission_lock_path(&self, generation: ParentLossGeneration) -> PathBuf {
        self.generation_path(generation).join("admissions")
    }

    pub fn generation_path(&self, generation: ParentLossGeneration) -> PathBuf {
        self.root_path()
            .join("generations")
            .join(generation.to_string())
    }

    pub fn record_path(&self, generation: ParentLossGeneration) -> PathBuf {
        self.generation_path(generation).join("record.json")
    }

    pub fn sealed_ledger_path(&self, generation: ParentLossGeneration) -> PathBuf {
        self.generation_path(generation).join("ledger.json")
    }

    pub fn active_generation(&self) -> Result<Option<ActiveGeneration>, ParentLossLedgerError> {
        read_json_optional(&self.active_path())
    }

    /// Reserve exactly one successor.  This never writes a transient pointer:
    /// no durable reservation means clean first-use retry; any returned error
    /// after a successful write leaves the confirmed pointer in place.
    pub fn reserve_generation(
        &self,
        supervisor: ProcessInstance,
        enabled: impl IntoIterator<Item = HostedServiceKind>,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        fs::create_dir_all(self.root_path())?;
        let active_path = self.active_path();
        let _lock = lifecycle_lock(&active_path)?;
        let next = match read_json_optional::<ActiveGeneration>(&active_path)? {
            None => 1,
            Some(active) => match self.outcome_for_active(&active)? {
                ParentLossReaderOutcome::Completed { generation, .. }
                | ParentLossReaderOutcome::RetiredExpected { generation }
                | ParentLossReaderOutcome::CancelledBeforeAdmission { generation } => {
                    generation + 1
                }
                ParentLossReaderOutcome::BootstrapRecoveryRequired { reason, .. } => {
                    return Err(ParentLossLedgerError::RecoveryRequired(reason));
                }
                _ => {
                    return Err(ParentLossLedgerError::RecoveryRequired(
                        BootstrapRecoveryReason::ActiveCoordinator,
                    ));
                }
            },
        };
        let mut enabled: Vec<_> = enabled.into_iter().collect();
        enabled.sort();
        enabled.dedup();
        let active = ActiveGeneration {
            schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
            generation: next,
            coordinator: None,
            supervisor,
            enabled,
            phase: ParentLossPhase::Allocated,
            reservation: BootstrapReservation::Confirmed,
        };
        write_json(&active_path, &active, json_options())?;
        Ok(active)
    }

    pub fn persist_coordinator_identity(
        &self,
        generation: ParentLossGeneration,
        coordinator: ProcessInstance,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        let active_path = self.active_path();
        let _active_lock = lifecycle_lock(&active_path)?;
        let Some(mut active) = read_json_optional::<ActiveGeneration>(&active_path)? else {
            return Err(ParentLossLedgerError::WrongGeneration { generation });
        };
        if active.generation != generation {
            return Err(ParentLossLedgerError::WrongGeneration { generation });
        }
        let record_path = self.record_path(generation);
        let _record_lock = lifecycle_lock(&record_path)?;
        let Some(mut record) = read_json_optional::<ParentLossGenerationRecord>(&record_path)?
        else {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::RecordMissing,
            ));
        };
        if record.generation != generation
            || record.coordinator.is_some_and(|value| value != coordinator)
        {
            return Err(ParentLossLedgerError::StaleCoordinator);
        }
        record.coordinator = Some(coordinator);
        write_json(&record_path, &record, json_options())?;
        active.coordinator = Some(coordinator);
        active.reservation = BootstrapReservation::CoordinatorIdentityPersisted;
        active.phase = ParentLossPhase::Allocated;
        write_json(&active_path, &active, json_options())?;
        Ok(active)
    }

    pub fn mark_admitting(
        &self,
        generation: ParentLossGeneration,
        coordinator: ProcessInstance,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        self.mutate_active(generation, |active| {
            require_coordinator(active, coordinator)?;
            active.phase = ParentLossPhase::Admitting;
            active.reservation = BootstrapReservation::InitialAdmissionPersisted;
            Ok(())
        })
    }

    pub fn seal(
        &self,
        generation: ParentLossGeneration,
        coordinator: ProcessInstance,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        self.mutate_active(generation, |active| {
            require_coordinator(active, coordinator)?;
            match active.phase {
                ParentLossPhase::Admitting => active.phase = ParentLossPhase::Sealed,
                // Preserve the acknowledgement until terminal publication so
                // the supervisor's bounded wait cannot miss it while the
                // coordinator seals and validates witness evidence.
                ParentLossPhase::RetiringAcknowledged => {}
                ParentLossPhase::Allocated
                | ParentLossPhase::Sealed
                | ParentLossPhase::Terminal => {
                    return Err(ParentLossLedgerError::RecoveryRequired(
                        BootstrapRecoveryReason::ReservationIncomplete,
                    ));
                }
            }
            Ok(())
        })
    }

    /// Durably linearize an authenticated graceful-retirement request.  Only
    /// the active coordinator may make this transition, and only before the
    /// ordinary sealing path has won.
    pub fn acknowledge_retirement(
        &self,
        generation: ParentLossGeneration,
        coordinator: ProcessInstance,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        self.mutate_active(generation, |active| {
            require_coordinator(active, coordinator)?;
            if active.phase != ParentLossPhase::Admitting {
                return Err(ParentLossLedgerError::RecoveryRequired(
                    BootstrapRecoveryReason::ReservationIncomplete,
                ));
            }
            active.phase = ParentLossPhase::RetiringAcknowledged;
            Ok(())
        })
    }

    pub fn initialize_record(
        &self,
        active: &ActiveGeneration,
    ) -> Result<(), ParentLossLedgerError> {
        let path = self.record_path(active.generation);
        fs::create_dir_all(path.parent().expect("generation record parent"))?;
        let _lock = lifecycle_lock(&path)?;
        if let Some(existing) = read_json_optional::<ParentLossGenerationRecord>(&path)? {
            if existing.generation == active.generation
                && existing.supervisor == active.supervisor
                && existing.coordinator == active.coordinator
            {
                return Ok(());
            }
            return Err(ParentLossLedgerError::WrongGeneration {
                generation: active.generation,
            });
        }
        write_json(
            &path,
            &ParentLossGenerationRecord {
                schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
                generation: active.generation,
                coordinator: active.coordinator,
                supervisor: active.supervisor,
                sealed_ledger_digest: None,
                terminal: None,
            },
            json_options(),
        )?;
        Ok(())
    }

    pub fn record(
        &self,
        generation: ParentLossGeneration,
    ) -> Result<Option<ParentLossGenerationRecord>, ParentLossLedgerError> {
        read_json_optional(&self.record_path(generation))
    }

    /// Write the immutable sealed ledger exactly once and return its SHA-256.
    pub(crate) fn write_sealed_ledger<T: Serialize>(
        &self,
        generation: ParentLossGeneration,
        entries: &T,
    ) -> Result<String, ParentLossLedgerError> {
        let path = self.sealed_ledger_path(generation);
        fs::create_dir_all(path.parent().expect("sealed ledger parent"))?;
        let _lock = lifecycle_lock(&path)?;
        let bytes = canonical_json(entries)?;
        let digest = digest_bytes(&bytes);
        if path.exists() {
            let existing = fs::read(&path)?;
            if digest_bytes(&existing) == digest {
                return Ok(digest);
            }
            return Err(ParentLossLedgerError::AlreadyTerminal);
        }
        write_json(&path, entries, json_options())?;
        Ok(digest)
    }

    pub(crate) fn write_terminal_with_digest(
        &self,
        generation: ParentLossGeneration,
        coordinator: ProcessInstance,
        disposition: ParentLossTerminalDisposition,
        sealed_ledger_digest: Option<String>,
    ) -> Result<(), ParentLossLedgerError> {
        let active_path = self.active_path();
        let _active_lock = lifecycle_lock(&active_path)?;
        let Some(mut active) = read_json_optional::<ActiveGeneration>(&active_path)? else {
            return Err(ParentLossLedgerError::WrongGeneration { generation });
        };
        if active.generation != generation {
            return Err(ParentLossLedgerError::WrongGeneration { generation });
        }
        require_coordinator(&active, coordinator)?;
        let record_path = self.record_path(generation);
        let _record_lock = lifecycle_lock(&record_path)?;
        let Some(mut record) = read_json_optional::<ParentLossGenerationRecord>(&record_path)?
        else {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::RecordMissing,
            ));
        };
        if record.generation != generation || record.coordinator != Some(coordinator) {
            return Err(ParentLossLedgerError::StaleCoordinator);
        }
        if record.terminal.is_some() {
            return Err(ParentLossLedgerError::AlreadyTerminal);
        }
        let Some(sealed_ledger_digest) = sealed_ledger_digest else {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::RecordMalformed,
            ));
        };
        let sealed = fs::read(self.sealed_ledger_path(generation))?;
        if digest_bytes(&sealed) != sealed_ledger_digest {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::RecordMalformed,
            ));
        }
        record.sealed_ledger_digest = Some(sealed_ledger_digest);
        record.terminal = Some(disposition);
        write_json(&record_path, &record, json_options())?;
        active.phase = ParentLossPhase::Terminal;
        write_json(&active_path, &active, json_options())?;
        Ok(())
    }

    pub fn outcome_for_active(
        &self,
        active: &ActiveGeneration,
    ) -> Result<ParentLossReaderOutcome, ParentLossLedgerError> {
        let Some(record) = self.record(active.generation)? else {
            return Ok(ParentLossReaderOutcome::BootstrapRecoveryRequired {
                generation: Some(active.generation),
                reason: BootstrapRecoveryReason::RecordMissing,
            });
        };
        if record.generation != active.generation {
            return Ok(ParentLossReaderOutcome::WrongGeneration {
                expected: active.generation,
                actual: record.generation,
            });
        }
        if let Some(outcome) = self.validate_terminal_artifacts(active, &record)? {
            return Ok(outcome);
        }
        match record.terminal {
            Some(ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest,
            }) => Ok(ParentLossReaderOutcome::Completed {
                generation: active.generation,
                sealed_ledger_digest,
            }),
            Some(ParentLossTerminalDisposition::Unresolved { reason }) => {
                Ok(ParentLossReaderOutcome::Unresolved {
                    generation: active.generation,
                    reason,
                })
            }
            Some(ParentLossTerminalDisposition::RetiredExpected) => {
                Ok(ParentLossReaderOutcome::RetiredExpected {
                    generation: active.generation,
                })
            }
            Some(ParentLossTerminalDisposition::CancelledBeforeAdmission) => {
                Ok(ParentLossReaderOutcome::CancelledBeforeAdmission {
                    generation: active.generation,
                })
            }
            None if active.coordinator.is_none() => {
                Ok(ParentLossReaderOutcome::BootstrapRecoveryRequired {
                    generation: Some(active.generation),
                    reason: BootstrapRecoveryReason::CoordinatorMissing,
                })
            }
            None => match SystemProcessInstanceSource
                .observe(&active.coordinator.expect("coordinator checked above"))
            {
                InstanceVerdict::SameLive { .. } => Ok(ParentLossReaderOutcome::Active {
                    generation: active.generation,
                    phase: active.phase,
                }),
                InstanceVerdict::NotSameOrExited | InstanceVerdict::Unverifiable => {
                    Ok(ParentLossReaderOutcome::BootstrapRecoveryRequired {
                        generation: Some(active.generation),
                        reason: BootstrapRecoveryReason::CoordinatorNotLive,
                    })
                }
            },
        }
    }

    fn mutate_active(
        &self,
        generation: ParentLossGeneration,
        mutate: impl FnOnce(&mut ActiveGeneration) -> Result<(), ParentLossLedgerError>,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        let path = self.active_path();
        let _lock = lifecycle_lock(&path)?;
        let Some(mut active) = read_json_optional::<ActiveGeneration>(&path)? else {
            return Err(ParentLossLedgerError::WrongGeneration { generation });
        };
        if active.generation != generation {
            return Err(ParentLossLedgerError::WrongGeneration { generation });
        }
        mutate(&mut active)?;
        write_json(&path, &active, json_options())?;
        Ok(active)
    }

    fn validate_terminal_artifacts(
        &self,
        active: &ActiveGeneration,
        record: &ParentLossGenerationRecord,
    ) -> Result<Option<ParentLossReaderOutcome>, ParentLossLedgerError> {
        let Some(terminal) = record.terminal.as_ref() else {
            return Ok(None);
        };
        let Some(expected_digest) = record.sealed_ledger_digest.as_ref() else {
            return Ok(Some(ParentLossReaderOutcome::ConflictingResult {
                artifact: "record.json".to_owned(),
            }));
        };
        let ledger_path = self.sealed_ledger_path(active.generation);
        let ledger = match fs::read(&ledger_path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Ok(Some(ParentLossReaderOutcome::MissingState {
                    artifact: "ledger.json".to_owned(),
                }));
            }
            Err(error) => return Err(error.into()),
        };
        if digest_bytes(&ledger) != *expected_digest {
            return Ok(Some(ParentLossReaderOutcome::ConflictingResult {
                artifact: "ledger.json".to_owned(),
            }));
        }
        if let ParentLossTerminalDisposition::Completed {
            sealed_ledger_digest,
        } = terminal
            && sealed_ledger_digest != expected_digest
        {
            return Ok(Some(ParentLossReaderOutcome::ConflictingResult {
                artifact: "record.json".to_owned(),
            }));
        }
        if matches!(
            terminal,
            ParentLossTerminalDisposition::Completed { .. }
                | ParentLossTerminalDisposition::RetiredExpected
        ) {
            let found: Vec<_> = active
                .enabled
                .iter()
                .copied()
                .filter(|service| {
                    self.generation_path(active.generation)
                        .join("witness")
                        .join(format!("{}.json", service_filename(*service)))
                        .is_file()
                })
                .collect();
            if found.len() != active.enabled.len() {
                return Ok(Some(ParentLossReaderOutcome::PartialWitnessSet {
                    generation: active.generation,
                    expected: active.enabled.clone(),
                    found,
                }));
            }
        }
        Ok(None)
    }
}

/// Read the current lifecycle without creating any state.
pub fn read_parent_loss_outcome(
    journal: &Path,
) -> Result<ParentLossReaderOutcome, ParentLossLedgerError> {
    let ledger = match ParentLossLedger::open(journal) {
        Ok(ledger) => ledger,
        Err(_) => {
            return Ok(ParentLossReaderOutcome::InaccessibleState {
                artifact: "journal-root".to_owned(),
            });
        }
    };
    let active = match ledger.active_generation() {
        Ok(active) => active,
        Err(ParentLossLedgerError::Json(_)) => {
            return Ok(ParentLossReaderOutcome::MalformedState {
                artifact: "active-generation.json".to_owned(),
            });
        }
        Err(ParentLossLedgerError::Io(_)) => {
            return Ok(ParentLossReaderOutcome::InaccessibleState {
                artifact: "active-generation.json".to_owned(),
            });
        }
        Err(error) => return Err(error),
    };
    let Some(active) = active else {
        return Ok(ParentLossReaderOutcome::CleanFirstUse);
    };
    match ledger.outcome_for_active(&active) {
        Ok(outcome) => Ok(outcome),
        Err(ParentLossLedgerError::Json(_)) => Ok(ParentLossReaderOutcome::MalformedState {
            artifact: "record.json".to_owned(),
        }),
        Err(ParentLossLedgerError::Io(error)) if error.kind() == io::ErrorKind::NotFound => {
            Ok(ParentLossReaderOutcome::MissingState {
                artifact: "record.json".to_owned(),
            })
        }
        Err(ParentLossLedgerError::Io(_)) => Ok(ParentLossReaderOutcome::InaccessibleState {
            artifact: "record.json".to_owned(),
        }),
        Err(error) => Err(error),
    }
}

fn require_coordinator(
    active: &ActiveGeneration,
    coordinator: ProcessInstance,
) -> Result<(), ParentLossLedgerError> {
    if active.coordinator == Some(coordinator) {
        Ok(())
    } else {
        Err(ParentLossLedgerError::StaleCoordinator)
    }
}

fn lifecycle_lock(
    path: &Path,
) -> Result<solstone_core_journal_io::FileLock, ParentLossLedgerError> {
    Ok(hold_lock(
        path,
        LockOptions {
            timeout: LOCK_TIMEOUT,
            poll_interval: Duration::from_millis(10),
            mode: Some(FILE_MODE),
        },
    )?)
}

fn json_options() -> JsonWriteOptions {
    JsonWriteOptions {
        mode: Some(FILE_MODE),
        indent: Some(2),
        sort_keys: true,
    }
}

fn read_json_optional<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<Option<T>, ParentLossLedgerError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

fn canonical_json<T: Serialize>(value: &T) -> Result<Vec<u8>, ParentLossLedgerError> {
    let mut value = serde_json::to_value(value)?;
    sort_json_value(&mut value);
    let mut bytes = serde_json::to_vec_pretty(&value)?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn sort_json_value(value: &mut serde_json::Value) {
    match value {
        serde_json::Value::Object(map) => {
            let old = std::mem::take(map);
            let mut ordered = std::collections::BTreeMap::new();
            for (key, mut value) in old {
                sort_json_value(&mut value);
                ordered.insert(key, value);
            }
            map.extend(ordered);
        }
        serde_json::Value::Array(values) => {
            for value in values {
                sort_json_value(value);
            }
        }
        _ => {}
    }
}

pub(crate) fn digest_bytes(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    format!("{digest:x}")
}

fn service_filename(service: HostedServiceKind) -> &'static str {
    match service {
        HostedServiceKind::Convey => "convey",
        HostedServiceKind::Sense => "sense",
        HostedServiceKind::Cortex => "cortex",
        HostedServiceKind::Spl => "spl",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lifecycle::{ParentLossServiceWitnessDrop, write_parent_loss_service_witness};
    use crate::process::ProcessBirth;
    use tempfile::TempDir;

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    fn ready_generation(
        directory: &TempDir,
        enabled: impl IntoIterator<Item = HostedServiceKind>,
    ) -> (ParentLossLedger, ActiveGeneration, ProcessInstance) {
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), enabled)
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let coordinator = instance(20, 2);
        let active = ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        let active = ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting state");
        (ledger, active, coordinator)
    }

    fn write_terminal(
        ledger: &ParentLossLedger,
        active: &ActiveGeneration,
        coordinator: ProcessInstance,
        terminal: ParentLossTerminalDisposition,
    ) {
        ledger.seal(active.generation, coordinator).expect("seal");
        let digest = ledger
            .write_sealed_ledger(active.generation, &Vec::<String>::new())
            .expect("sealed ledger");
        ledger
            .write_terminal_with_digest(active.generation, coordinator, terminal, Some(digest))
            .expect("terminal write");
    }

    fn witness(
        generation: ParentLossGeneration,
        service: HostedServiceKind,
    ) -> ParentLossServiceWitnessDrop {
        let identity = crate::lifecycle::AdmissionIdentity {
            generation,
            launch_id: format!("{service:?}").to_lowercase(),
            instance: instance(30, 3),
            uid: 501,
            parent_launch_id: None,
        };
        ParentLossServiceWitnessDrop {
            schema: 1,
            service,
            parent: instance(10, 1),
            identity,
            listener_stopped: true,
            service_runner_stopped: true,
            operational_artifacts_cleaned: true,
            descendants_retired: true,
            shutdown_complete: true,
            descendant_failure: None,
        }
    }

    fn matching_completed_outcome(directory: &TempDir) -> ParentLossReaderOutcome {
        let (ledger, active, coordinator) = ready_generation(directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest: "fixture".to_owned(),
            },
        );
        let mut record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        let digest = record.sealed_ledger_digest.clone().expect("digest");
        record.terminal = Some(ParentLossTerminalDisposition::Completed {
            sealed_ledger_digest: digest.clone(),
        });
        write_json(
            &ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("matching completed record");
        ParentLossReaderOutcome::Completed {
            generation: active.generation,
            sealed_ledger_digest: digest,
        }
    }

    #[test]
    fn allocation_and_terminal_transition_are_generation_fenced() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [HostedServiceKind::Sense])
            .expect("reserve");
        assert_eq!(active.generation, 1);
        ledger.initialize_record(&active).expect("record");
        let coordinator = instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("identity");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting");
        ledger.seal(active.generation, coordinator).expect("seal");
        let digest = ledger
            .write_sealed_ledger(active.generation, &vec!["one"])
            .expect("ledger");
        ledger
            .write_terminal_with_digest(
                active.generation,
                coordinator,
                ParentLossTerminalDisposition::CancelledBeforeAdmission,
                Some(digest.clone()),
            )
            .expect("terminal");
        assert!(matches!(
            ledger.write_terminal_with_digest(
                active.generation,
                coordinator,
                ParentLossTerminalDisposition::CancelledBeforeAdmission,
                Some(digest)
            ),
            Err(ParentLossLedgerError::AlreadyTerminal)
        ));
        let second = ledger
            .reserve_generation(instance(11, 3), [HostedServiceKind::Sense])
            .expect("successor");
        assert_eq!(second.generation, 2);
    }

    #[test]
    fn reader_outcomes_cover_clean_terminal_and_blocked_states() {
        let clean = TempDir::new().expect("temporary root");
        assert_eq!(
            read_parent_loss_outcome(clean.path()).expect("clean read"),
            ParentLossReaderOutcome::CleanFirstUse
        );

        let completed = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&completed, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest: "ignored by writer".to_owned(),
            },
        );
        let record = ledger
            .record(active.generation)
            .expect("record")
            .expect("completed record");
        let digest = record.sealed_ledger_digest.expect("digest");
        assert_eq!(
            read_parent_loss_outcome(completed.path()).expect("completed read"),
            ParentLossReaderOutcome::ConflictingResult {
                artifact: "record.json".to_owned()
            }
        );
        // A matching digest is the only completed success.
        let mut record = ledger
            .record(active.generation)
            .expect("record")
            .expect("completed record");
        record.terminal = Some(ParentLossTerminalDisposition::Completed {
            sealed_ledger_digest: digest.clone(),
        });
        write_json(
            &ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("repair test fixture");
        assert_eq!(
            read_parent_loss_outcome(completed.path()).expect("matching completed read"),
            ParentLossReaderOutcome::Completed {
                generation: active.generation,
                sealed_ledger_digest: digest,
            }
        );

        for terminal in [
            ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::WitnessConflict,
            },
            ParentLossTerminalDisposition::RetiredExpected,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        ] {
            let directory = TempDir::new().expect("temporary root");
            let (ledger, active, coordinator) = ready_generation(&directory, []);
            write_terminal(&ledger, &active, coordinator, terminal.clone());
            let outcome = read_parent_loss_outcome(directory.path()).expect("reader outcome");
            match terminal {
                ParentLossTerminalDisposition::Unresolved { reason } => assert_eq!(
                    outcome,
                    ParentLossReaderOutcome::Unresolved {
                        generation: active.generation,
                        reason,
                    }
                ),
                ParentLossTerminalDisposition::RetiredExpected => assert_eq!(
                    outcome,
                    ParentLossReaderOutcome::RetiredExpected {
                        generation: active.generation,
                    }
                ),
                ParentLossTerminalDisposition::CancelledBeforeAdmission => assert_eq!(
                    outcome,
                    ParentLossReaderOutcome::CancelledBeforeAdmission {
                        generation: active.generation,
                    }
                ),
                ParentLossTerminalDisposition::Completed { .. } => unreachable!(),
            }
        }
    }

    #[test]
    fn reader_refuses_missing_malformed_wrong_and_conflicting_artifacts() {
        let missing_record = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(missing_record.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reserve");
        assert_eq!(
            read_parent_loss_outcome(missing_record.path()).expect("missing record read"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired {
                generation: Some(active.generation),
                reason: BootstrapRecoveryReason::RecordMissing,
            }
        );

        std::fs::write(ledger.active_path(), b"not-json").expect("malformed active fixture");
        assert_eq!(
            read_parent_loss_outcome(missing_record.path()).expect("malformed read"),
            ParentLossReaderOutcome::MalformedState {
                artifact: "active-generation.json".to_owned(),
            }
        );

        let wrong_generation = TempDir::new().expect("temporary root");
        let (ledger, active, _coordinator) = ready_generation(&wrong_generation, []);
        let mut record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        record.generation += 1;
        write_json(
            &ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("wrong-generation fixture");
        assert_eq!(
            read_parent_loss_outcome(wrong_generation.path()).expect("wrong generation read"),
            ParentLossReaderOutcome::WrongGeneration {
                expected: active.generation,
                actual: active.generation + 1,
            }
        );

        let conflicting = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&conflicting, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        );
        std::fs::write(
            ledger.sealed_ledger_path(active.generation),
            b"other-ledger\n",
        )
        .expect("conflicting ledger fixture");
        assert_eq!(
            read_parent_loss_outcome(conflicting.path()).expect("conflicting read"),
            ParentLossReaderOutcome::ConflictingResult {
                artifact: "ledger.json".to_owned(),
            }
        );
        let _ = coordinator;
    }

    #[test]
    fn completed_reader_requires_the_full_witness_set() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(
            &directory,
            [HostedServiceKind::Convey, HostedServiceKind::Sense],
        );
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest: "fixture".to_owned(),
            },
        );
        let record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        let mut matching_record = record.clone();
        matching_record.terminal = Some(ParentLossTerminalDisposition::Completed {
            sealed_ledger_digest: record.sealed_ledger_digest.clone().expect("digest"),
        });
        write_json(
            &ledger.record_path(active.generation),
            &matching_record,
            json_options(),
        )
        .expect("matching record fixture");
        write_parent_loss_service_witness(
            directory.path(),
            &witness(active.generation, HostedServiceKind::Convey),
        )
        .expect("one witness");
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("partial witness read"),
            ParentLossReaderOutcome::PartialWitnessSet {
                generation: active.generation,
                expected: vec![HostedServiceKind::Convey, HostedServiceKind::Sense],
                found: vec![HostedServiceKind::Convey],
            }
        );
        write_parent_loss_service_witness(
            directory.path(),
            &witness(active.generation, HostedServiceKind::Sense),
        )
        .expect("second witness");
        assert!(matches!(
            read_parent_loss_outcome(directory.path()).expect("complete witness read"),
            ParentLossReaderOutcome::Completed { .. }
        ));
    }

    #[test]
    fn stale_coordinator_cannot_change_a_later_generation() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, old_coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            old_coordinator,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        );
        let next = ledger
            .reserve_generation(instance(11, 3), [])
            .expect("successor");
        ledger.initialize_record(&next).expect("successor record");
        let new_coordinator = instance(21, 4);
        ledger
            .persist_coordinator_identity(next.generation, new_coordinator)
            .expect("new coordinator");
        ledger
            .mark_admitting(next.generation, new_coordinator)
            .expect("new admission");
        ledger
            .seal(next.generation, new_coordinator)
            .expect("new seal");
        let digest = ledger
            .write_sealed_ledger(next.generation, &Vec::<String>::new())
            .expect("new ledger");
        assert!(matches!(
            ledger.write_terminal_with_digest(
                next.generation,
                old_coordinator,
                ParentLossTerminalDisposition::CancelledBeforeAdmission,
                Some(digest),
            ),
            Err(ParentLossLedgerError::StaleCoordinator)
        ));
    }

    #[test]
    fn reservation_fault_seams_preserve_clean_or_blocked_state() {
        let before_reservation = TempDir::new().expect("temporary root");
        assert_eq!(
            read_parent_loss_outcome(before_reservation.path()).expect("clean retry"),
            ParentLossReaderOutcome::CleanFirstUse
        );

        let after_reservation = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(after_reservation.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("confirmed reservation");
        assert_eq!(
            ledger
                .outcome_for_active(&active)
                .expect("reservation outcome"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired {
                generation: Some(active.generation),
                reason: BootstrapRecoveryReason::RecordMissing,
            }
        );
        ledger.initialize_record(&active).expect("record seam");
        assert_eq!(
            ledger.outcome_for_active(&active).expect("identity seam"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired {
                generation: Some(active.generation),
                reason: BootstrapRecoveryReason::CoordinatorMissing,
            }
        );
        let coordinator = instance(20, 2);
        let active = ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("identity persistence seam");
        assert!(matches!(
            ledger.outcome_for_active(&active).expect("admission seam"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired {
                reason: BootstrapRecoveryReason::CoordinatorNotLive,
                ..
            }
        ));
        assert!(
            ledger.reserve_generation(instance(11, 3), []).is_err(),
            "durable nonterminal state never self-releases"
        );
    }

    #[cfg(unix)]
    #[test]
    fn canonical_path_aliases_share_one_lifecycle_domain() {
        use std::os::unix::fs::symlink;

        let directory = TempDir::new().expect("temporary root");
        let alias_parent = TempDir::new().expect("temporary alias parent");
        let alias = alias_parent.path().join("journal-alias");
        symlink(directory.path(), &alias).expect("journal alias");
        let first = ParentLossLedger::open(directory.path()).expect("canonical ledger");
        let second = ParentLossLedger::open(&alias).expect("alias ledger");
        assert_eq!(first.canonical_root(), second.canonical_root());
        first
            .reserve_generation(instance(10, 1), [])
            .expect("first authority reservation");
        assert!(second.reserve_generation(instance(11, 2), []).is_err());
    }

    #[test]
    fn distinct_roots_allocate_independent_first_generations() {
        let first = TempDir::new().expect("first root");
        let second = TempDir::new().expect("second root");
        assert_eq!(
            ParentLossLedger::open(first.path())
                .expect("first ledger")
                .reserve_generation(instance(10, 1), [])
                .expect("first generation")
                .generation,
            1
        );
        assert_eq!(
            ParentLossLedger::open(second.path())
                .expect("second ledger")
                .reserve_generation(instance(11, 2), [])
                .expect("second generation")
                .generation,
            1
        );
    }

    #[test]
    fn matching_completed_is_the_only_terminal_success_for_admitted_work() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::Completed {
                sealed_ledger_digest: "mismatch".to_owned(),
            },
        );
        let mut record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        let digest = record.sealed_ledger_digest.clone().expect("sealed digest");
        record.terminal = Some(ParentLossTerminalDisposition::Completed {
            sealed_ledger_digest: digest.clone(),
        });
        write_json(
            &ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("matching completed fixture");
        let completed = read_parent_loss_outcome(directory.path()).expect("completed outcome");
        assert!(matches!(
            completed,
            ParentLossReaderOutcome::Completed { .. }
        ));

        for terminal in [
            ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::ArtifactFailure,
            },
            ParentLossTerminalDisposition::RetiredExpected,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        ] {
            let directory = TempDir::new().expect("temporary root");
            let (ledger, active, coordinator) = ready_generation(&directory, []);
            write_terminal(&ledger, &active, coordinator, terminal);
            assert!(
                !matches!(
                    read_parent_loss_outcome(directory.path()).expect("terminal outcome"),
                    ParentLossReaderOutcome::Completed { .. }
                ),
                "only a digest-valid Completed result may admit successor service work"
            );
        }
    }

    #[test]
    fn reader_reports_missing_state_when_a_terminal_ledger_disappears() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        );
        std::fs::remove_file(ledger.sealed_ledger_path(active.generation))
            .expect("remove sealed ledger fixture");
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("missing ledger outcome"),
            ParentLossReaderOutcome::MissingState {
                artifact: "ledger.json".to_owned(),
            }
        );
    }

    #[test]
    fn reader_reports_inaccessible_state_for_a_non_directory_journal_root() {
        let directory = TempDir::new().expect("temporary parent");
        let file = directory.path().join("not-a-journal");
        std::fs::write(&file, b"not a directory").expect("file fixture");
        assert!(matches!(
            read_parent_loss_outcome(&file).expect("inaccessible root outcome"),
            ParentLossReaderOutcome::InaccessibleState { .. }
        ));
    }

    #[test]
    fn malformed_and_mismatched_companion_records_block_fresh_allocation() {
        let malformed = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(malformed.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reservation");
        std::fs::create_dir_all(ledger.generation_path(active.generation)).expect("generation dir");
        std::fs::write(ledger.record_path(active.generation), b"not-json")
            .expect("malformed record fixture");
        assert_eq!(
            read_parent_loss_outcome(malformed.path()).expect("malformed companion outcome"),
            ParentLossReaderOutcome::MalformedState {
                artifact: "record.json".to_owned(),
            }
        );
        assert!(ledger.reserve_generation(instance(11, 2), []).is_err());

        let mismatched = TempDir::new().expect("temporary root");
        let (ledger, active, _) = ready_generation(&mismatched, []);
        let mut record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        record.generation += 1;
        write_json(
            &ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("mismatched record fixture");
        assert!(matches!(
            read_parent_loss_outcome(mismatched.path()).expect("mismatched outcome"),
            ParentLossReaderOutcome::WrongGeneration { .. }
        ));
        assert!(ledger.reserve_generation(instance(11, 2), []).is_err());
    }

    #[test]
    fn inaccessible_companion_record_blocks_fresh_allocation_without_side_effects() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reservation");
        std::fs::create_dir_all(ledger.record_path(active.generation))
            .expect("inaccessible record-directory fixture");
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("inaccessible companion outcome"),
            ParentLossReaderOutcome::InaccessibleState {
                artifact: "record.json".to_owned(),
            }
        );
        assert!(ledger.reserve_generation(instance(11, 2), []).is_err());
        assert_eq!(
            ledger
                .active_generation()
                .expect("active generation")
                .expect("active generation remains")
                .generation,
            active.generation
        );
    }

    #[test]
    fn concurrent_terminal_attempts_have_one_immutable_winner() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        ledger.seal(active.generation, coordinator).expect("seal");
        let digest = ledger
            .write_sealed_ledger(active.generation, &Vec::<String>::new())
            .expect("sealed ledger");
        let path = Arc::new(directory.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for terminal in [
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
            ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::ArtifactFailure,
            },
        ] {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            let digest = digest.clone();
            handles.push(thread::spawn(move || {
                let ledger = ParentLossLedger::open(&path).expect("thread ledger");
                barrier.wait();
                ledger.write_terminal_with_digest(1, coordinator, terminal, Some(digest))
            }));
        }
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("terminal thread"))
            .collect();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert_eq!(
            outcomes
                .iter()
                .filter(|outcome| matches!(outcome, Err(ParentLossLedgerError::AlreadyTerminal)))
                .count(),
            1
        );
    }

    #[test]
    fn concurrent_first_reservations_admit_exactly_one_generation() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let directory = TempDir::new().expect("temporary root");
        let path = Arc::new(directory.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for pid in [10, 11] {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let ledger = ParentLossLedger::open(&path).expect("thread ledger");
                barrier.wait();
                ledger.reserve_generation(instance(pid, u64::from(pid)), [HostedServiceKind::Sense])
            }));
        }
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("reservation thread"))
            .collect();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert_eq!(
            ParentLossLedger::open(directory.path())
                .expect("ledger")
                .active_generation()
                .expect("active generation")
                .expect("one active generation")
                .generation,
            1
        );
    }

    #[test]
    fn reader_outcome_clean_first_use_is_explicit() {
        let directory = TempDir::new().expect("temporary root");
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("clean outcome"),
            ParentLossReaderOutcome::CleanFirstUse
        );
    }

    #[test]
    fn reader_outcome_completed_requires_a_matching_digest() {
        let directory = TempDir::new().expect("temporary root");
        let expected = matching_completed_outcome(&directory);
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("completed outcome"),
            expected
        );
    }

    #[test]
    fn reader_outcome_unresolved_is_durable() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::WitnessConflict,
            },
        );
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("unresolved outcome"),
            ParentLossReaderOutcome::Unresolved {
                generation: active.generation,
                reason: ParentLossUnresolvedReason::WitnessConflict,
            }
        );
    }

    #[test]
    fn reader_outcome_bootstrap_recovery_required_is_durable() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reservation");
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("bootstrap recovery outcome"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired {
                generation: Some(active.generation),
                reason: BootstrapRecoveryReason::RecordMissing,
            }
        );
    }

    #[test]
    fn reader_outcome_wrong_generation_is_explicit() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, _) = ready_generation(&directory, []);
        let mut record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        record.generation += 1;
        write_json(
            &ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("wrong generation record");
        assert!(matches!(
            read_parent_loss_outcome(directory.path()).expect("wrong generation outcome"),
            ParentLossReaderOutcome::WrongGeneration { .. }
        ));
    }

    #[test]
    fn reader_outcome_conflicting_result_is_explicit() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        );
        std::fs::write(ledger.sealed_ledger_path(active.generation), b"conflict\n")
            .expect("conflicting ledger");
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("conflict outcome"),
            ParentLossReaderOutcome::ConflictingResult {
                artifact: "ledger.json".to_owned(),
            }
        );
    }

    #[test]
    fn reader_outcome_retired_expected_is_durable() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::RetiredExpected,
        );
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("retired expected outcome"),
            ParentLossReaderOutcome::RetiredExpected {
                generation: active.generation,
            }
        );
    }

    #[test]
    fn reader_outcome_cancelled_before_admission_is_durable() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active, coordinator) = ready_generation(&directory, []);
        write_terminal(
            &ledger,
            &active,
            coordinator,
            ParentLossTerminalDisposition::CancelledBeforeAdmission,
        );
        assert_eq!(
            read_parent_loss_outcome(directory.path()).expect("cancelled outcome"),
            ParentLossReaderOutcome::CancelledBeforeAdmission {
                generation: active.generation,
            }
        );
    }

    #[test]
    fn validated_completed_successor_race_allocates_exactly_one_generation() {
        use std::sync::{Arc, Barrier};
        use std::thread;

        let directory = TempDir::new().expect("temporary root");
        assert!(matches!(
            matching_completed_outcome(&directory),
            ParentLossReaderOutcome::Completed { .. }
        ));
        let path = Arc::new(directory.path().to_path_buf());
        let barrier = Arc::new(Barrier::new(2));
        let mut handles = Vec::new();
        for pid in [800, 801] {
            let path = Arc::clone(&path);
            let barrier = Arc::clone(&barrier);
            handles.push(thread::spawn(move || {
                let ledger = ParentLossLedger::open(&path).expect("successor ledger");
                barrier.wait();
                ledger.reserve_generation(instance(pid, u64::from(pid)), [])
            }));
        }
        let outcomes: Vec<_> = handles
            .into_iter()
            .map(|handle| handle.join().expect("successor thread"))
            .collect();
        assert_eq!(outcomes.iter().filter(|outcome| outcome.is_ok()).count(), 1);
        assert_eq!(
            ParentLossLedger::open(directory.path())
                .expect("fresh reader ledger")
                .active_generation()
                .expect("active generation")
                .expect("successor active")
                .generation,
            2
        );
    }
}
