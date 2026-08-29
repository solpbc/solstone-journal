// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable, single-owner evidence drops for hosted child admission.
//!
//! Launchers own `intent` and `result`; each hosted service owns only its own
//! acknowledgement and witness.  The coordinator reads these files and is the
//! sole writer of the generation ledger and terminal record.

use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::os::unix::fs::OpenOptionsExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use thiserror::Error;

use super::HostedServiceKind;
use super::parent_loss_ledger::{ParentLossGeneration, ParentLossLedger, ParentLossLedgerError};
use crate::process::ProcessInstance;
use crate::process::{InspectResult, ProcessInstanceSource, SystemProcessInstanceSource};

const SCHEMA: u32 = 1;
const FILE_MODE: u32 = 0o600;
pub const HOSTED_GENERATION_ENV: &str = "SOL_PARENT_LOSS_GENERATION";
pub const HOSTED_LAUNCH_ID_ENV: &str = "SOL_PARENT_LOSS_LAUNCH_ID";
pub const HOSTED_PARENT_LAUNCH_ID_ENV: &str = "SOL_PARENT_LOSS_PARENT_LAUNCH_ID";

/// Exact authority required before a coordinator can retire an admitted child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionIdentity {
    pub generation: ParentLossGeneration,
    pub launch_id: String,
    pub instance: ProcessInstance,
    pub uid: u32,
    pub parent_launch_id: Option<String>,
}

/// Intent written before the process exists, while the admission lock is held.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionIntent {
    pub schema: u32,
    pub generation: ParentLossGeneration,
    pub launch_id: String,
    pub service: Option<HostedServiceKind>,
    pub parent_launch_id: Option<String>,
}

/// The launcher-owned post-spawn outcome.  It is written once, after the
/// child acknowledges (or after the exact launch boundary reaps it).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionResult {
    pub schema: u32,
    pub identity: Option<AdmissionIdentity>,
    pub state: AdmissionResultState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionResultState {
    Admitted,
    SpawnFailed {
        detail: String,
    },
    RejectedAndReaped {
        exit_code: Option<i32>,
    },
    /// The launch boundary rejected the child but could not prove its exact
    /// PID/birth had been reaped.  The coordinator must fail the generation
    /// closed rather than treating this like a harmless rejected launch.
    RejectedUnreaped {
        detail: String,
    },
}

/// Child-owned durable acknowledgement, compared by the parent launch
/// boundary to its direct exact spawn observation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionAcknowledgement {
    pub schema: u32,
    pub identity: AdmissionIdentity,
}

/// Service-owned evidence. A repeated byte-equivalent witness is harmless;
/// a divergent witness remains on disk for the coordinator to diagnose.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentLossServiceWitnessDrop {
    pub schema: u32,
    pub service: HostedServiceKind,
    pub parent: ProcessInstance,
    pub identity: AdmissionIdentity,
    pub listener_stopped: bool,
    pub service_runner_stopped: bool,
    pub operational_artifacts_cleaned: bool,
    pub descendants_retired: bool,
    pub shutdown_complete: bool,
    pub descendant_failure: Option<crate::process::DescendantObservationFailure>,
}

#[derive(Debug, Error)]
pub enum ParentLossAdmissionError {
    #[error(transparent)]
    Ledger(#[from] ParentLossLedgerError),
    #[error("parent-loss admission I/O failed: {0}")]
    Io(#[from] io::Error),
    #[error("parent-loss admission JSON failed: {0}")]
    Json(#[from] serde_json::Error),
    #[error("admission drop conflicts at {artifact}")]
    Conflict { artifact: String },
    #[error("hosted admission environment is missing or malformed")]
    MissingProvenance,
    #[error("hosted admission acknowledgement did not match this process")]
    IdentityMismatch,
    #[error("hosted admission launch ID must be a single safe path segment")]
    InvalidLaunchId,
}

impl AdmissionIntent {
    pub fn new(
        generation: ParentLossGeneration,
        launch_id: impl Into<String>,
        service: Option<HostedServiceKind>,
        parent_launch_id: Option<String>,
    ) -> Self {
        Self {
            schema: SCHEMA,
            generation,
            launch_id: launch_id.into(),
            service,
            parent_launch_id,
        }
    }
}

pub fn write_parent_loss_admission_intent(
    journal: &Path,
    intent: &AdmissionIntent,
) -> Result<(), ParentLossAdmissionError> {
    validate_launch_id(&intent.launch_id)?;
    let ledger = ParentLossLedger::open(journal)?;
    write_immutable_json(
        &intent_path(&ledger, intent.generation, &intent.launch_id),
        intent,
    )
}

pub fn write_parent_loss_admission_result(
    journal: &Path,
    generation: ParentLossGeneration,
    launch_id: &str,
    result: &AdmissionResult,
) -> Result<(), ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    let ledger = ParentLossLedger::open(journal)?;
    write_immutable_json(&result_path(&ledger, generation, launch_id), result)
}

pub fn read_parent_loss_admission_acknowledgement(
    journal: &Path,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<Option<AdmissionAcknowledgement>, ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    let ledger = ParentLossLedger::open(journal)?;
    read_json_optional(&ack_path(&ledger, generation, launch_id))
}

/// Host-service entry points call this before listener binding or readiness.
pub fn acknowledge_parent_loss_admission(
    journal: &Path,
    identity: AdmissionIdentity,
) -> Result<(), ParentLossAdmissionError> {
    validate_launch_id(&identity.launch_id)?;
    let ledger = ParentLossLedger::open(journal)?;
    let acknowledgement = AdmissionAcknowledgement {
        schema: SCHEMA,
        identity: identity.clone(),
    };
    write_immutable_json(
        &ack_path(&ledger, identity.generation, &identity.launch_id),
        &acknowledgement,
    )
}

/// A short-lived hosted child calls this at its process entry before doing
/// work. Unlike a hosted service, it does not own a supervisor watcher or a
/// service witness; it only completes its launch boundary's durable admission
/// acknowledgement.
pub fn acknowledge_hosted_child_admission(journal: &Path) -> Result<(), ParentLossAdmissionError> {
    if std::env::var_os(HOSTED_GENERATION_ENV).is_none()
        && std::env::var_os(HOSTED_LAUNCH_ID_ENV).is_none()
    {
        return Ok(());
    }
    let source = SystemProcessInstanceSource;
    let (instance, uid) = match source.inspect(std::process::id()) {
        InspectResult::Present { instance, uid, .. } => (instance, uid),
        InspectResult::Absent | InspectResult::Unverifiable => {
            return Err(ParentLossAdmissionError::MissingProvenance);
        }
    };
    acknowledge_parent_loss_admission(journal, parse_hosted_admission_environment(instance, uid)?)
}

pub fn write_parent_loss_service_witness(
    journal: &Path,
    witness: &ParentLossServiceWitnessDrop,
) -> Result<(), ParentLossAdmissionError> {
    let ledger = ParentLossLedger::open(journal)?;
    let path = witness_path(&ledger, witness.identity.generation, witness.service);
    write_immutable_json(&path, witness)
}

pub(crate) fn read_parent_loss_admission_intent(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<Option<AdmissionIntent>, ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    read_json_optional(&intent_path(ledger, generation, launch_id))
}

pub(crate) fn read_parent_loss_admission_result(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<Option<AdmissionResult>, ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    read_json_optional(&result_path(ledger, generation, launch_id))
}

pub(crate) fn admission_directory(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> PathBuf {
    ledger.generation_path(generation).join("admissions")
}

pub(crate) fn witness_path(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    service: HostedServiceKind,
) -> PathBuf {
    ledger
        .generation_path(generation)
        .join("witness")
        .join(format!("{}.json", service_name(service)))
}

pub(crate) fn service_name(service: HostedServiceKind) -> &'static str {
    match service {
        HostedServiceKind::Convey => "convey",
        HostedServiceKind::Sense => "sense",
        HostedServiceKind::Cortex => "cortex",
        HostedServiceKind::Spl => "spl",
    }
}

pub(crate) fn intent_path(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> PathBuf {
    admission_directory(ledger, generation)
        .join(launch_id)
        .join("intent.json")
}

pub(crate) fn result_path(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> PathBuf {
    admission_directory(ledger, generation)
        .join(launch_id)
        .join("result.json")
}

pub(crate) fn ack_path(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> PathBuf {
    admission_directory(ledger, generation)
        .join(launch_id)
        .join("acknowledgement.json")
}

pub(crate) fn parse_hosted_admission_environment(
    identity: ProcessInstance,
    uid: u32,
) -> Result<AdmissionIdentity, ParentLossAdmissionError> {
    let generation = std::env::var(HOSTED_GENERATION_ENV)
        .ok()
        .and_then(|value| value.parse().ok())
        .ok_or(ParentLossAdmissionError::MissingProvenance)?;
    let launch_id = std::env::var(HOSTED_LAUNCH_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty())
        .ok_or(ParentLossAdmissionError::MissingProvenance)?;
    validate_launch_id(&launch_id)?;
    let parent_launch_id = std::env::var(HOSTED_PARENT_LAUNCH_ID_ENV)
        .ok()
        .filter(|value| !value.is_empty());
    Ok(AdmissionIdentity {
        generation,
        launch_id,
        instance: identity,
        uid,
        parent_launch_id,
    })
}

/// Admission drops are addressed by `launch_id`, so reject anything that is
/// not one path component before it can reach a `PathBuf::join` call.
pub(crate) fn validate_launch_id(launch_id: &str) -> Result<(), ParentLossAdmissionError> {
    if launch_id.is_empty()
        || !launch_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(ParentLossAdmissionError::InvalidLaunchId);
    }
    Ok(())
}

fn write_immutable_json<T: Serialize + for<'de> Deserialize<'de> + PartialEq>(
    path: &Path,
    value: &T,
) -> Result<(), ParentLossAdmissionError> {
    let parent = path.parent().expect("drop has parent");
    fs::create_dir_all(parent)?;
    let bytes = serde_json::to_vec_pretty(value)?;
    let mut bytes = bytes;
    bytes.push(b'\n');
    let mut options = OpenOptions::new();
    options.write(true).create_new(true).mode(FILE_MODE);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
            let existing: T = serde_json::from_slice(&fs::read(path)?)?;
            if &existing == value {
                Ok(())
            } else {
                Err(ParentLossAdmissionError::Conflict {
                    artifact: path.display().to_string(),
                })
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn read_json_optional<T: for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<Option<T>, ParentLossAdmissionError> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(serde_json::from_slice(&bytes)?)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error.into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::process::ProcessBirth;
    use tempfile::TempDir;

    fn identity() -> AdmissionIdentity {
        AdmissionIdentity {
            generation: 7,
            launch_id: "launch-a".to_owned(),
            instance: ProcessInstance {
                pid: 42,
                birth: ProcessBirth::linux(9, 1, 100),
            },
            uid: 501,
            parent_launch_id: None,
        }
    }

    #[test]
    fn same_witness_is_idempotent_and_divergent_witness_conflicts() {
        let directory = TempDir::new().expect("temporary root");
        let witness = ParentLossServiceWitnessDrop {
            schema: SCHEMA,
            service: HostedServiceKind::Sense,
            parent: identity().instance,
            identity: identity(),
            listener_stopped: true,
            service_runner_stopped: true,
            operational_artifacts_cleaned: true,
            descendants_retired: true,
            shutdown_complete: true,
            descendant_failure: None,
        };
        write_parent_loss_service_witness(directory.path(), &witness).expect("first");
        write_parent_loss_service_witness(directory.path(), &witness).expect("same");
        let mut conflicting = witness;
        conflicting.listener_stopped = false;
        assert!(matches!(
            write_parent_loss_service_witness(directory.path(), &conflicting),
            Err(ParentLossAdmissionError::Conflict { .. })
        ));
    }

    #[test]
    fn launcher_drops_are_immutable_per_launch_and_idempotent_when_identical() {
        let directory = TempDir::new().expect("temporary root");
        let intent = AdmissionIntent::new(7, "launch-a", Some(HostedServiceKind::Sense), None);
        write_parent_loss_admission_intent(directory.path(), &intent).expect("first intent");
        write_parent_loss_admission_intent(directory.path(), &intent).expect("same intent");
        let conflicting_intent =
            AdmissionIntent::new(7, "launch-a", Some(HostedServiceKind::Cortex), None);
        assert!(matches!(
            write_parent_loss_admission_intent(directory.path(), &conflicting_intent),
            Err(ParentLossAdmissionError::Conflict { .. })
        ));

        let result = AdmissionResult {
            schema: SCHEMA,
            identity: Some(identity()),
            state: AdmissionResultState::Admitted,
        };
        write_parent_loss_admission_result(directory.path(), 7, "launch-a", &result)
            .expect("first result");
        write_parent_loss_admission_result(directory.path(), 7, "launch-a", &result)
            .expect("same result");
        let conflicting_result = AdmissionResult {
            schema: SCHEMA,
            identity: Some(identity()),
            state: AdmissionResultState::RejectedAndReaped { exit_code: Some(9) },
        };
        assert!(matches!(
            write_parent_loss_admission_result(
                directory.path(),
                7,
                "launch-a",
                &conflicting_result,
            ),
            Err(ParentLossAdmissionError::Conflict { .. })
        ));
    }

    #[test]
    fn acknowledgement_binds_generation_pid_birth_uid_and_parent_launch() {
        let directory = TempDir::new().expect("temporary root");
        let mut expected = identity();
        expected.parent_launch_id = Some("parent-launch".to_owned());
        acknowledge_parent_loss_admission(directory.path(), expected.clone())
            .expect("matching acknowledgement");
        acknowledge_parent_loss_admission(directory.path(), expected.clone())
            .expect("idempotent acknowledgement");

        let mut wrong_uid = expected;
        wrong_uid.uid += 1;
        assert!(matches!(
            acknowledge_parent_loss_admission(directory.path(), wrong_uid),
            Err(ParentLossAdmissionError::Conflict { .. })
        ));
    }

    #[test]
    fn independent_launch_ids_cannot_overwrite_each_others_admission_drops() {
        let directory = TempDir::new().expect("temporary root");
        let first = AdmissionIntent::new(7, "launch-a", Some(HostedServiceKind::Sense), None);
        let second = AdmissionIntent::new(7, "launch-b", Some(HostedServiceKind::Cortex), None);
        write_parent_loss_admission_intent(directory.path(), &first).expect("first intent");
        write_parent_loss_admission_intent(directory.path(), &second).expect("second intent");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        assert_eq!(
            read_parent_loss_admission_intent(&ledger, 7, "launch-a").expect("read first"),
            Some(first)
        );
        assert_eq!(
            read_parent_loss_admission_intent(&ledger, 7, "launch-b").expect("read second"),
            Some(second)
        );
    }

    #[test]
    fn missing_or_partial_environment_never_constructs_admission_provenance() {
        assert!(matches!(
            parse_hosted_admission_environment(identity().instance, identity().uid),
            Err(ParentLossAdmissionError::MissingProvenance)
        ));
    }

    #[test]
    fn traversal_launch_id_is_rejected_before_creating_an_admission_drop() {
        let directory = TempDir::new().expect("temporary root");
        let intent = AdmissionIntent::new(
            7,
            "../../outside-admissions",
            Some(HostedServiceKind::Sense),
            None,
        );

        assert!(matches!(
            write_parent_loss_admission_intent(directory.path(), &intent),
            Err(ParentLossAdmissionError::InvalidLaunchId)
        ));
        assert!(
            !directory.path().join("health").exists(),
            "invalid launch IDs must fail before the admission writer creates paths"
        );
    }
}
