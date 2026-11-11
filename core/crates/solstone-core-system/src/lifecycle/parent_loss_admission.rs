// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Immutable, single-owner evidence drops for hosted child admission.
//!
//! Launchers own `intent` and `result`; each hosted service owns only its own
//! acknowledgement and witness.  The coordinator reads these files and is the
//! sole writer of the generation ledger and terminal record.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{
    AtomicWriteOptions, FileLock, FinalNameConfirmation, LockOptions, MetadataDurability,
    hold_lock, write_bytes_exclusive_detailed,
};
use thiserror::Error;

use super::HostedServiceKind;
use super::parent_loss_ledger::{
    ParentLossGeneration, ParentLossLedger, ParentLossLedgerError, ParentLossPhase,
};
use crate::process::ProcessInstance;
use crate::process::{
    HostedLaunchProvenance, InspectResult, InstanceVerdict, ProcessInstanceSource,
    SystemProcessInstanceSource,
};

const SCHEMA: u32 = 1;
const FILE_MODE: u32 = 0o600;
/// How long a reaper or launcher waits for the admission lock before leaving
/// the entry for the seal. Contention costs speed, never safety.
pub const DISCHARGE_LOCK_TIMEOUT: Duration = Duration::from_millis(100);
const SEALING_DISCHARGE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
pub const HOSTED_GENERATION_ENV: &str = "SOL_PARENT_LOSS_GENERATION";
pub const HOSTED_LAUNCH_ID_ENV: &str = "SOL_PARENT_LOSS_LAUNCH_ID";
pub const HOSTED_PARENT_LAUNCH_ID_ENV: &str = "SOL_PARENT_LOSS_PARENT_LAUNCH_ID";

/// A random 64-bit launch id. Launch ids are never reused, so a launch is
/// refused rather than given a predictable id when randomness fails.
pub fn generate_helper_launch_id(prefix: &str) -> io::Result<String> {
    let mut nonce = [0u8; 8];
    getrandom::fill(&mut nonce)
        .map_err(|error| io::Error::other(format!("launch id randomness failed: {error}")))?;
    let mut hex = String::with_capacity(16);
    for byte in nonce {
        use std::fmt::Write;
        let _ = write!(&mut hex, "{byte:02x}");
    }
    Ok(format!("{prefix}-{hex}"))
}

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

/// Immutable spawn-identity drop written immediately after inspect before
/// waiting for child acknowledgement. Ensures dead generations never leave
/// mid-ack spawned children unattributable.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionSpawnIdentity {
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
    #[error("the generation this child was launched under no longer admits")]
    GenerationClosed,
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
        DropParent::Create,
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
    write_immutable_json(
        &result_path(&ledger, generation, launch_id),
        result,
        DropParent::MustExist,
    )
}

pub fn write_parent_loss_admission_spawn_identity(
    journal: &Path,
    identity: &AdmissionIdentity,
) -> Result<(), ParentLossAdmissionError> {
    validate_launch_id(&identity.launch_id)?;
    let ledger = ParentLossLedger::open(journal)?;
    let drop = AdmissionSpawnIdentity {
        schema: SCHEMA,
        identity: identity.clone(),
    };
    write_immutable_json(
        &spawn_identity_path(&ledger, identity.generation, &identity.launch_id),
        &drop,
        DropParent::MustExist,
    )
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

pub fn read_parent_loss_admission_spawn_identity(
    journal: &Path,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<Option<AdmissionSpawnIdentity>, ParentLossAdmissionError> {
    let ledger = ParentLossLedger::open(journal)?;
    read_parent_loss_admission_spawn_identity_in(&ledger, generation, launch_id)
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
        DropParent::MustExist,
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
    let identity = parse_hosted_admission_environment(instance, uid)?;
    acknowledge_parent_loss_admission(journal, identity.clone())?;
    require_generation_still_admitting(journal, identity.generation)
}

/// After acknowledging, confirm the generation still admits. A launcher that
/// died mid-launch leaves this child unrecorded; the start that closes the
/// abandoned generation seals its pointer BEFORE scanning admissions, so an
/// acknowledgement written after that scan meets a sealed pointer here and
/// the child does no work against a journal a successor now owns.
pub(crate) fn require_generation_still_admitting(
    journal: &Path,
    generation: ParentLossGeneration,
) -> Result<(), ParentLossAdmissionError> {
    let ledger = ParentLossLedger::open(journal)?;
    let admitting = ledger.active_generation()?.is_some_and(|active| {
        active.generation == generation
            && active.phase == super::parent_loss_ledger::ParentLossPhase::Admitting
    });
    if admitting {
        Ok(())
    } else {
        Err(ParentLossAdmissionError::GenerationClosed)
    }
}

/// Derive a new descendant launch from this process's acknowledged admission.
/// Environment inherited from a different process is not launch authority.
/// The launch boundary still owns the generation seal check and child admission.
pub fn hosted_child_launch_provenance(
    launch_id: String,
    acknowledgement_timeout: Duration,
) -> Result<Option<HostedLaunchProvenance>, ParentLossAdmissionError> {
    if [
        HOSTED_GENERATION_ENV,
        HOSTED_LAUNCH_ID_ENV,
        HOSTED_PARENT_LAUNCH_ID_ENV,
    ]
    .iter()
    .all(|name| std::env::var_os(name).is_none())
    {
        return Ok(None);
    }
    validate_launch_id(&launch_id)?;
    let journal = std::env::var_os("SOLSTONE_JOURNAL")
        .map(PathBuf::from)
        .ok_or(ParentLossAdmissionError::MissingProvenance)?;
    let InspectResult::Present { instance, uid, .. } =
        SystemProcessInstanceSource.inspect(std::process::id())
    else {
        return Err(ParentLossAdmissionError::MissingProvenance);
    };
    let identity = parse_hosted_admission_environment(instance, uid)?;
    let acknowledgement = read_parent_loss_admission_acknowledgement(
        &journal,
        identity.generation,
        &identity.launch_id,
    )?
    .ok_or(ParentLossAdmissionError::MissingProvenance)?;
    if acknowledgement.schema != SCHEMA || acknowledgement.identity != identity {
        return Err(ParentLossAdmissionError::IdentityMismatch);
    }
    Ok(Some(HostedLaunchProvenance {
        journal,
        generation: identity.generation,
        launch_id,
        service: None,
        parent_launch_id: Some(identity.launch_id),
        acknowledgement_timeout,
    }))
}

pub fn write_parent_loss_service_witness(
    journal: &Path,
    witness: &ParentLossServiceWitnessDrop,
) -> Result<(), ParentLossAdmissionError> {
    let ledger = ParentLossLedger::open(journal)?;
    let path = witness_path(&ledger, witness.identity.generation, witness.service);
    write_immutable_json(&path, witness, DropParent::Create)
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

pub(crate) fn read_parent_loss_admission_spawn_identity_in(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<Option<AdmissionSpawnIdentity>, ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    read_json_optional(&spawn_identity_path(ledger, generation, launch_id))
}

/// Who is discharging an admission entry. Each has its own precondition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discharger {
    /// The owner that reaped the child, holding its exit status. Only for a
    /// non-service entry of the admitting generation with a readable intent.
    Reaper,
    /// A launcher, under the lock it already holds, for a launch that left no
    /// process or for a replaced service root it observed exited.
    Launcher,
    /// The coordinator or closer after the seal.
    Sealing,
}

/// The outcome of a discharge. Anything but `Moved` leaves the entry in
/// `admissions/` for the seal to settle by observation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Discharged {
    Moved,
    Kept(DischargeKept),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DischargeKept {
    LockBusy,
    PointerUnreadable,
    NotActiveGeneration,
    NotAdmitting,
    IntentUnreadable,
    ServiceRoot,
    EntryMissing,
    RenameFailed,
}

/// Where a discharged entry goes: `generations/<g>/discharged/<launch_id>`.
pub(crate) fn discharged_directory(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> PathBuf {
    ledger.generation_path(generation).join("discharged")
}

/// The one choke point that removes an entry from the outstanding set. It
/// takes the admission lock with a short timeout; on contention the entry is
/// left for the seal.
pub fn discharge(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
    discharger: Discharger,
) -> Result<Discharged, ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    let Ok(lock) = hold_lock(
        ledger.admission_lock_path(generation),
        LockOptions {
            // After the seal only the sealing authority discharges, so its wait
            // is contended only by reapers that are about to give up.
            timeout: if discharger == Discharger::Sealing {
                SEALING_DISCHARGE_LOCK_TIMEOUT
            } else {
                DISCHARGE_LOCK_TIMEOUT
            },
            poll_interval: Duration::from_millis(10),
            mode: Some(FILE_MODE),
        },
    ) else {
        return Ok(Discharged::Kept(DischargeKept::LockBusy));
    };
    discharge_holding_lock(ledger, generation, launch_id, discharger, &lock)
}

/// [`discharge`] for a caller that already holds this generation's admission
/// lock. flock is not reentrant, so the lock is never taken again here.
pub fn discharge_holding_lock(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
    discharger: Discharger,
    _held: &FileLock,
) -> Result<Discharged, ParentLossAdmissionError> {
    validate_launch_id(launch_id)?;
    if discharger != Discharger::Sealing {
        match ledger.active_generation() {
            Ok(Some(active)) if active.generation == generation => {
                if active.phase != ParentLossPhase::Admitting {
                    return Ok(Discharged::Kept(DischargeKept::NotAdmitting));
                }
            }
            Ok(_) => return Ok(Discharged::Kept(DischargeKept::NotActiveGeneration)),
            Err(_) => return Ok(Discharged::Kept(DischargeKept::PointerUnreadable)),
        }
    }
    let entry = admission_directory(ledger, generation).join(launch_id);
    if discharger == Discharger::Reaper {
        match read_parent_loss_admission_intent(ledger, generation, launch_id) {
            Ok(Some(intent)) if intent.schema == SCHEMA => {
                if intent.service.is_some() {
                    return Ok(Discharged::Kept(DischargeKept::ServiceRoot));
                }
            }
            Ok(None) if !entry.exists() => {
                return Ok(Discharged::Kept(DischargeKept::EntryMissing));
            }
            _ => return Ok(Discharged::Kept(DischargeKept::IntentUnreadable)),
        }
    }
    let target = discharged_directory(ledger, generation);
    match fs::create_dir(&target) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
        Err(_) => return Ok(Discharged::Kept(DischargeKept::RenameFailed)),
    }
    match fs::rename(&entry, target.join(launch_id)) {
        Ok(()) => Ok(Discharged::Moved),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            Ok(Discharged::Kept(DischargeKept::EntryMissing))
        }
        // The trash can still hold this name from an interrupted earlier
        // discharge the prune has not reached: keep both.
        Err(_) if target.join(launch_id).exists() => {
            match fs::rename(
                &entry,
                target.join(format!("{launch_id}.{}", trash_suffix())),
            ) {
                Ok(()) => Ok(Discharged::Moved),
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    Ok(Discharged::Kept(DischargeKept::EntryMissing))
                }
                Err(_) => Ok(Discharged::Kept(DischargeKept::RenameFailed)),
            }
        }
        Err(_) => Ok(Discharged::Kept(DischargeKept::RenameFailed)),
    }
}

fn trash_suffix() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

/// The admission entry a launched child belongs to, carried by the object
/// that reaps it so the reap discharges the entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdmissionTicket {
    pub journal: PathBuf,
    pub generation: ParentLossGeneration,
    pub launch_id: String,
}

impl AdmissionTicket {
    /// Called once, after `Child::try_wait()` returned an exit status.
    pub(crate) fn discharge_reaped(self) -> Discharged {
        let Ok(ledger) = ParentLossLedger::open(&self.journal) else {
            return Discharged::Kept(DischargeKept::PointerUnreadable);
        };
        discharge(
            &ledger,
            self.generation,
            &self.launch_id,
            Discharger::Reaper,
        )
        .unwrap_or(Discharged::Kept(DischargeKept::IntentUnreadable))
    }
}

/// After a replacement launch of `service` wrote its own intent, discharge
/// every other root of that service in the generation whose every identity
/// is observed exited. A root with an unreadable drop, no identity, or any
/// identity that is live or unobservable is left for the seal. The caller
/// holds the admission lock.
pub(crate) fn discharge_replaced_service_roots(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    current_launch_id: &str,
    service: HostedServiceKind,
    source: &dyn ProcessInstanceSource,
    held: &FileLock,
) -> usize {
    let Ok(listing) = fs::read_dir(admission_directory(ledger, generation)) else {
        return 0;
    };
    let names: Vec<String> = listing
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .filter(|name| name != current_launch_id && validate_launch_id(name).is_ok())
        .collect();
    let mut discharged = 0;
    for launch_id in names {
        match read_parent_loss_admission_intent(ledger, generation, &launch_id) {
            Ok(Some(intent)) if intent.schema == SCHEMA && intent.service == Some(service) => {}
            _ => continue,
        }
        let Some(identities) = entry_identities(ledger, generation, &launch_id) else {
            continue;
        };
        let all_exited = !identities.is_empty()
            && identities.iter().all(|identity| {
                matches!(
                    source.observe(&identity.instance),
                    InstanceVerdict::NotSameOrExited
                )
            });
        if all_exited
            && discharge_holding_lock(ledger, generation, &launch_id, Discharger::Launcher, held)
                .is_ok_and(|outcome| outcome == Discharged::Moved)
        {
            discharged += 1;
        }
    }
    discharged
}

/// Every identity an entry's drops yield, or `None` when any present drop is
/// unreadable or has another schema.
fn entry_identities(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Option<Vec<AdmissionIdentity>> {
    let mut identities = Vec::new();
    match read_parent_loss_admission_result(ledger, generation, launch_id) {
        Ok(Some(result)) if result.schema == SCHEMA => identities.extend(result.identity),
        Ok(None) => {}
        _ => return None,
    }
    match read_parent_loss_admission_spawn_identity_in(ledger, generation, launch_id) {
        Ok(Some(drop)) if drop.schema == SCHEMA => identities.push(drop.identity),
        Ok(None) => {}
        _ => return None,
    }
    match read_json_optional::<AdmissionAcknowledgement>(&ack_path(ledger, generation, launch_id)) {
        Ok(Some(drop)) if drop.schema == SCHEMA => identities.push(drop.identity),
        Ok(None) => {}
        _ => return None,
    }
    Some(identities)
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

pub fn service_name(service: HostedServiceKind) -> &'static str {
    match service {
        HostedServiceKind::Convey => "convey",
        HostedServiceKind::Sense => "sense",
        HostedServiceKind::Cortex => "cortex",
        HostedServiceKind::Spl => "spl",
        HostedServiceKind::Mcp => "mcp",
        HostedServiceKind::McpLocalDoor => "mcp_local_door",
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

pub(crate) fn spawn_identity_path(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> PathBuf {
    admission_directory(ledger, generation)
        .join(launch_id)
        .join("spawn-identity.json")
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

/// Whether a drop may create its parent directory. Only an intent (written
/// under the admission lock) creates an admission entry, and a witness lives
/// outside `admissions/`. Every other drop writes into an existing entry, so a
/// discharged entry is never recreated.
#[derive(Clone, Copy, PartialEq, Eq)]
enum DropParent {
    Create,
    MustExist,
}

/// Publish one immutable drop: stage in the same directory, fsync, then a
/// no-replace publish, so the final name never holds a partial file. An
/// identical existing drop is accepted; a different one is a conflict.
fn write_immutable_json<T: Serialize + for<'de> Deserialize<'de> + PartialEq>(
    path: &Path,
    value: &T,
    parent: DropParent,
) -> Result<(), ParentLossAdmissionError> {
    if parent == DropParent::Create {
        fs::create_dir_all(path.parent().expect("drop has parent"))?;
    }
    let mut bytes = serde_json::to_vec_pretty(value)?;
    bytes.push(b'\n');
    let options = AtomicWriteOptions {
        mode: Some(FILE_MODE),
    };
    let error = match write_bytes_exclusive_detailed(path, &bytes, options) {
        Ok(publication) => {
            if let FinalNameConfirmation::Unverified { reason, .. } = publication.final_name {
                return Err(reason.into());
            }
            if let MetadataDurability::Unproven {
                source: Some(source),
            } = publication.durability
            {
                return Err(source.into());
            }
            return Ok(());
        }
        Err(error) => error,
    };
    match error.source.kind() {
        io::ErrorKind::AlreadyExists
            if matches!(error.operation, "inspect destination" | "publish stage") =>
        {
            let existing: T = serde_json::from_slice(&fs::read(path)?)?;
            if &existing == value {
                Ok(())
            } else {
                Err(ParentLossAdmissionError::Conflict {
                    artifact: path.display().to_string(),
                })
            }
        }
        io::ErrorKind::NotFound if parent == DropParent::MustExist => {
            Err(ParentLossAdmissionError::GenerationClosed)
        }
        _ => Err(error.source.into()),
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

    fn admitting(directory: &Path) -> (ParentLossLedger, ParentLossGeneration, ProcessInstance) {
        let ledger = ParentLossLedger::open(directory).expect("ledger");
        let supervisor = ProcessInstance {
            pid: 10,
            birth: ProcessBirth::linux(1, 1, 100),
        };
        let active = ledger
            .reserve_generation(supervisor, [HostedServiceKind::Sense])
            .expect("reserve");
        ledger.initialize_record(&active).expect("record");
        let coordinator = ProcessInstance {
            pid: 20,
            birth: ProcessBirth::linux(2, 1, 100),
        };
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator");
        ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting");
        (ledger, active.generation, coordinator)
    }

    fn entry(ledger: &ParentLossLedger, generation: ParentLossGeneration, id: &str) -> PathBuf {
        admission_directory(ledger, generation).join(id)
    }

    fn worker_identity(generation: ParentLossGeneration, id: &str, pid: u32) -> AdmissionIdentity {
        AdmissionIdentity {
            generation,
            launch_id: id.to_owned(),
            instance: ProcessInstance {
                pid,
                birth: ProcessBirth::linux(u64::from(pid), 1, 100),
            },
            uid: 501,
            parent_launch_id: None,
        }
    }

    #[test]
    fn a_reaped_worker_leaves_admissions_and_a_late_drop_cannot_recreate_it() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, g, _) = admitting(directory.path());
        let journal = directory.path();
        write_parent_loss_admission_intent(journal, &AdmissionIntent::new(g, "w", None, None))
            .expect("intent");
        assert_eq!(
            discharge(&ledger, g, "w", Discharger::Reaper).expect("discharge"),
            Discharged::Moved
        );
        assert!(!entry(&ledger, g, "w").exists());
        assert!(
            discharged_directory(&ledger, g)
                .join("w/intent.json")
                .is_file()
        );

        let id = worker_identity(g, "w", 4242);
        assert!(matches!(
            write_parent_loss_admission_spawn_identity(journal, &id),
            Err(ParentLossAdmissionError::GenerationClosed)
        ));
        assert!(matches!(
            acknowledge_parent_loss_admission(journal, id.clone()),
            Err(ParentLossAdmissionError::GenerationClosed)
        ));
        assert!(matches!(
            write_parent_loss_admission_result(
                journal,
                g,
                "w",
                &AdmissionResult {
                    schema: SCHEMA,
                    identity: Some(id),
                    state: AdmissionResultState::Admitted,
                },
            ),
            Err(ParentLossAdmissionError::GenerationClosed)
        ));
        assert!(
            !entry(&ledger, g, "w").exists(),
            "a drop after discharge recreated the entry"
        );
    }

    #[test]
    fn the_reaper_leaves_service_roots_unreadable_intents_and_sealed_generations() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, g, coordinator) = admitting(directory.path());
        let journal = directory.path();
        write_parent_loss_admission_intent(
            journal,
            &AdmissionIntent::new(g, "svc", Some(HostedServiceKind::Sense), None),
        )
        .expect("service intent");
        assert_eq!(
            discharge(&ledger, g, "svc", Discharger::Reaper).expect("discharge"),
            Discharged::Kept(DischargeKept::ServiceRoot)
        );

        // A torn intent and an intent with another schema are unreadable.
        fs::create_dir_all(entry(&ledger, g, "torn")).expect("torn entry");
        fs::write(entry(&ledger, g, "torn").join("intent.json"), b"{\"sche").expect("torn");
        let mut other = serde_json::to_value(AdmissionIntent::new(g, "other", None, None))
            .expect("intent value");
        other["schema"] = serde_json::json!(2);
        fs::create_dir_all(entry(&ledger, g, "other")).expect("other entry");
        fs::write(
            entry(&ledger, g, "other").join("intent.json"),
            serde_json::to_vec(&other).expect("bytes"),
        )
        .expect("schema 2");
        fs::create_dir_all(entry(&ledger, g, "bare")).expect("intent-less entry");
        for id in ["torn", "other", "bare"] {
            assert_eq!(
                discharge(&ledger, g, id, Discharger::Reaper).expect("discharge"),
                Discharged::Kept(DischargeKept::IntentUnreadable),
                "{id}"
            );
            assert!(entry(&ledger, g, id).exists(), "{id}");
        }
        assert_eq!(
            discharge(&ledger, g, "never-written", Discharger::Reaper).expect("discharge"),
            Discharged::Kept(DischargeKept::EntryMissing)
        );

        write_parent_loss_admission_intent(journal, &AdmissionIntent::new(g, "w", None, None))
            .expect("intent");
        ledger.seal(g, coordinator).expect("seal");
        for discharger in [Discharger::Reaper, Discharger::Launcher] {
            assert_eq!(
                discharge(&ledger, g, "w", discharger).expect("discharge"),
                Discharged::Kept(DischargeKept::NotAdmitting)
            );
        }
        assert!(entry(&ledger, g, "w").exists());
        assert_eq!(
            discharge(&ledger, g, "w", Discharger::Sealing).expect("discharge"),
            Discharged::Moved
        );
    }

    #[test]
    fn a_reaper_that_cannot_take_the_lock_leaves_the_entry() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, g, _) = admitting(directory.path());
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(g, "w", None, None),
        )
        .expect("intent");
        let _held = hold_lock(
            ledger.admission_lock_path(g),
            LockOptions {
                timeout: Duration::from_secs(1),
                poll_interval: Duration::from_millis(10),
                mode: Some(FILE_MODE),
            },
        )
        .expect("lock");
        let began = std::time::Instant::now();
        assert_eq!(
            discharge(&ledger, g, "w", Discharger::Reaper).expect("discharge"),
            Discharged::Kept(DischargeKept::LockBusy)
        );
        assert!(began.elapsed() < Duration::from_secs(1));
        assert!(entry(&ledger, g, "w").exists());
    }

    #[cfg(feature = "test-hooks")]
    #[test]
    fn a_drop_interrupted_mid_write_leaves_no_partial_file_under_its_final_name() {
        use solstone_core_journal_io::{
            BoundPublicationPrimitive, run_with_bound_publication_barrier,
            run_with_bound_publication_fault,
        };
        let directory = TempDir::new().expect("temporary root");
        let (ledger, g, _) = admitting(directory.path());
        let journal = directory.path().to_path_buf();
        write_parent_loss_admission_intent(&journal, &AdmissionIntent::new(g, "w", None, None))
            .expect("intent");
        let id = worker_identity(g, "w", 4242);
        let final_path = spawn_identity_path(&ledger, g, "w");

        // Fail the write of the staged bytes: the final name never appears and
        // the stage is cleaned up.
        let (result, injected) = run_with_bound_publication_fault(
            BoundPublicationPrimitive::Write,
            1,
            libc::EIO,
            || write_parent_loss_admission_spawn_identity(&journal, &id),
        );
        assert!(injected && result.is_err());
        assert!(!final_path.exists());
        let names: Vec<_> = fs::read_dir(entry(&ledger, g, "w"))
            .expect("entry")
            .map(|item| item.expect("item").file_name())
            .collect();
        assert_eq!(names, vec![std::ffi::OsString::from("intent.json")]);

        // Fully written and synced, but not yet published: still no final name.
        let observed = final_path.clone();
        let (result, fired) = run_with_bound_publication_barrier(
            BoundPublicationPrimitive::ExclusivePublish,
            1,
            move || assert!(!observed.exists(), "final name visible before publish"),
            || write_parent_loss_admission_spawn_identity(&journal, &id),
        );
        assert!(fired);
        result.expect("published");
        let read: AdmissionSpawnIdentity =
            serde_json::from_slice(&fs::read(&final_path).expect("final")).expect("whole drop");
        assert_eq!(read.identity, id);
    }

    #[test]
    fn a_replacement_discharges_only_roots_whose_every_identity_exited() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, g, _) = admitting(directory.path());
        let journal = directory.path();
        let service = HostedServiceKind::Sense;
        let live = worker_identity(g, "x", 7001).instance;
        let root = |id: &str, kind: HostedServiceKind| {
            write_parent_loss_admission_intent(
                journal,
                &AdmissionIntent::new(g, id, Some(kind), None),
            )
            .expect("intent");
        };
        let result = |id: &str, identity: AdmissionIdentity| {
            write_parent_loss_admission_result(
                journal,
                g,
                id,
                &AdmissionResult {
                    schema: SCHEMA,
                    identity: Some(identity),
                    state: AdmissionResultState::Admitted,
                },
            )
            .expect("result");
        };
        // exited: one identity, gone.
        root("exited", service);
        result("exited", worker_identity(g, "exited", 6001));
        // disagree: the result is gone, but its acknowledgement names a live process.
        root("disagree", service);
        result("disagree", worker_identity(g, "disagree", 6002));
        let mut ack = worker_identity(g, "disagree", 7001);
        ack.instance = live;
        acknowledge_parent_loss_admission(journal, ack).expect("ack");
        // live: the root is still running.
        root("live", service);
        let mut running = worker_identity(g, "live", 7001);
        running.instance = live;
        result("live", running);
        // intent-only: no identity to observe.
        root("intent-only", service);
        // torn: an unreadable drop beside an exited identity.
        root("torn", service);
        result("torn", worker_identity(g, "torn", 6003));
        fs::write(entry(&ledger, g, "torn").join("acknowledgement.json"), b"{").expect("torn");
        // another service, exited.
        root("cortex", HostedServiceKind::Cortex);
        result("cortex", worker_identity(g, "cortex", 6004));
        // the replacement itself.
        root("current", service);

        struct OnlyLive(ProcessInstance);
        impl ProcessInstanceSource for OnlyLive {
            fn inspect(&self, pid: u32) -> InspectResult {
                if pid == self.0.pid {
                    InspectResult::Present {
                        instance: self.0,
                        uid: 501,
                        execution: crate::process::ExecutionState::Running,
                        ppid: Some(1),
                        pgid: Some(1),
                    }
                } else {
                    InspectResult::Absent
                }
            }
            fn census(&self) -> crate::process::InstanceCensus {
                crate::process::InstanceCensus::Incomplete(Vec::new())
            }
        }
        let lock = hold_lock(
            ledger.admission_lock_path(g),
            LockOptions {
                timeout: Duration::from_secs(1),
                poll_interval: Duration::from_millis(10),
                mode: Some(FILE_MODE),
            },
        )
        .expect("lock");
        let moved = discharge_replaced_service_roots(
            &ledger,
            g,
            "current",
            service,
            &OnlyLive(live),
            &lock,
        );
        assert_eq!(moved, 1);
        assert!(!entry(&ledger, g, "exited").exists());
        for kept in [
            "disagree",
            "live",
            "intent-only",
            "torn",
            "cortex",
            "current",
        ] {
            assert!(entry(&ledger, g, kept).exists(), "{kept} was discharged");
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
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(7, "launch-a", None, None),
        )
        .expect("intent");
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
    fn spawn_identity_is_immutable_and_idempotent() {
        let directory = TempDir::new().expect("temporary root");
        let id = identity();
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(7, "launch-a", None, None),
        )
        .expect("intent");
        write_parent_loss_admission_spawn_identity(directory.path(), &id)
            .expect("first spawn identity");
        write_parent_loss_admission_spawn_identity(directory.path(), &id)
            .expect("idempotent spawn identity");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let read =
            read_parent_loss_admission_spawn_identity_in(&ledger, id.generation, &id.launch_id)
                .expect("read spawn identity");
        assert_eq!(
            read,
            Some(AdmissionSpawnIdentity {
                schema: SCHEMA,
                identity: id.clone()
            })
        );

        let mut conflicting = id.clone();
        conflicting.uid += 1;
        assert!(matches!(
            write_parent_loss_admission_spawn_identity(directory.path(), &conflicting),
            Err(ParentLossAdmissionError::Conflict { .. })
        ));
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
