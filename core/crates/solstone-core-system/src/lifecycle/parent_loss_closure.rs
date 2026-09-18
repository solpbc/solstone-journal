// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closing a generation whose supervisor and coordinator both exited before
//! the coordinator could publish a terminal result.
//!
//! The incident shape: a service manager's stop timeout SIGKILLs the whole
//! control group, or the machine loses power. The supervisor and its
//! coordinator die together, the generation stays open, and every later start
//! refused it as `CoordinatorNotLive` until an operator moved
//! `health/parent-loss` aside by hand.
//!
//! Liveness here is proven by lock, and only corroborated by identity. The
//! caller holds the coordinator lease (`ParentLossLedger::acquire_coordinator_lease`,
//! an advisory `flock` the kernel releases on death of any kind) and its parent
//! supervisor holds the supervisor singleton lock, so neither recorded
//! authority can still be running. Both identities are still observed, and a
//! `SameLive` observation refuses. ⛔ "Cannot observe" is never read as death
//! on its own: the lock is the proof, the observation is the veto.
//!
//! Every process the dead generation admitted has an exact identity in its
//! `admissions/` directory. Each one is proven exited, retired here with an
//! exact signal, or the start refuses with a reason that converges once the
//! process is gone. Two instances of a service never run against one journal.

use std::fs;
use std::io;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::{LockOptions, hold_lock, write_json};

use super::HostedServiceKind;
use super::parent_loss_admission::{
    AdmissionResultState, admission_directory, read_parent_loss_admission_acknowledgement_in,
    read_parent_loss_admission_intent, read_parent_loss_admission_result,
};
use super::parent_loss_coordinator::{SealedAdmission, clear_supervisor_heartbeat};
use super::parent_loss_ledger::{
    BootstrapRecoveryReason, PARENT_LOSS_LEDGER_SCHEMA_V1, ParentLossGeneration,
    ParentLossGenerationRecord, ParentLossLedger, ParentLossLedgerError,
    ParentLossTerminalDisposition, ParentLossUnresolvedReason, digest_bytes, json_options,
};
use crate::process::{
    InstanceVerdict, ProcessInstance, ProcessInstanceSource, ProcessOwner, SignalKind,
    SystemProcessInstanceSource, TerminationError, process_owner, signal_exact_instance,
};

pub const PARENT_LOSS_CLOSURE_SCHEMA_V1: u32 = 1;
/// How long a transient `Unverifiable` (a pid exiting between two procfs
/// reads) is re-observed before it is recorded as unobservable.
const OBSERVATION_SETTLE_WINDOW: Duration = Duration::from_millis(500);
const OBSERVATION_POLL: Duration = Duration::from_millis(25);
const ADMISSION_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// Time kept back from the deadline for SIGKILL escalation and its reap.
const ESCALATION_RESERVE: Duration = Duration::from_millis(1_500);

/// How a recorded authority was found when the generation was closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthorityObservation {
    /// The recorded pid+birth observed `NotSameOrExited`.
    Exited,
    /// The recorded pid could not be inspected within the settle window. The
    /// lock it would have held was free, which is the proof relied on; the
    /// observation is recorded, not acted on.
    Unobservable,
    /// The reservation died before this identity was ever persisted.
    NeverRecorded,
}

/// What became of one admission the dead generation recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AdmissionFinding {
    /// The exact identity observed `NotSameOrExited` before any signal.
    Exited,
    /// The exact identity was still live and was signalled here.
    Retired {
        escalated: bool,
    },
    /// An intent with neither a result nor an acknowledgement: the child never
    /// completed admission, so it never bound a listener or served.
    NeverAcknowledged,
    SpawnFailed,
    RejectedAndReaped,
    /// The pid could not be inspected, and the process table says it now
    /// belongs to another user: a reused pid, not ours. A positive
    /// observation, never an inference from "cannot tell".
    PidReusedByAnotherUser,
    /// The drop carried no identity we could act on.
    Unidentified,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdmissionClosure {
    pub launch_id: String,
    pub service: Option<HostedServiceKind>,
    pub instance: Option<ProcessInstance>,
    /// The uid the admitted process ran as, from its admission identity.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub uid: Option<u32>,
    pub finding: AdmissionFinding,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HeartbeatRetirement {
    Removed,
    Absent,
    /// The record predates the heartbeat field; the stale-heartbeat collector
    /// retires it after its own floor.
    NotRecorded,
    NotCleared,
}

/// The durable account of a closure, written into the closed generation's
/// record beside its terminal disposition. `journal doctor` names these.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AbandonedGenerationClosure {
    pub schema: u32,
    /// The successor coordinator that closed it.
    pub closed_by: ProcessInstance,
    pub successor_generation: ParentLossGeneration,
    /// Unix seconds.
    pub closed_at: u64,
    pub supervisor: AuthorityObservation,
    pub coordinator: AuthorityObservation,
    pub heartbeat: HeartbeatRetirement,
    /// Bookkeeping files set aside beside the record, never deleted.
    pub set_aside: Vec<String>,
    /// Anomalies worth a human's eye, in plain words.
    pub notes: Vec<String>,
    pub admissions: Vec<AdmissionClosure>,
}

/// Sends exact signals during retirement and answers who owns a pid.
/// Injected so every refuse path is testable without a live process to kill.
pub trait AdmissionRetirer: Send + Sync {
    fn signal(&self, instance: ProcessInstance, signal: SignalKind)
    -> Result<(), TerminationError>;
    fn owner(&self, pid: u32) -> ProcessOwner;
}

/// Production retirer: every signal is guarded by a fresh exact observation.
#[derive(Debug, Clone, Copy, Default)]
pub struct SystemAdmissionRetirer;

impl AdmissionRetirer for SystemAdmissionRetirer {
    fn signal(
        &self,
        instance: ProcessInstance,
        signal: SignalKind,
    ) -> Result<(), TerminationError> {
        signal_exact_instance(instance, signal, &SystemProcessInstanceSource)
    }

    fn owner(&self, pid: u32) -> ProcessOwner {
        process_owner(pid)
    }
}

/// What the closer needs beyond the ledger and the lease it is called under.
pub struct ClosingAuthority<'a> {
    /// The successor coordinator's own identity.
    pub closed_by: ProcessInstance,
    pub source: &'a dyn ProcessInstanceSource,
    pub retirer: &'a dyn AdmissionRetirer,
    /// Retirement must finish by here or the start refuses.
    pub deadline: Instant,
}

/// The generation being closed, as the caller found it under the active lock.
pub(crate) struct AbandonedGeneration {
    pub generation: ParentLossGeneration,
    pub supervisor: Option<ProcessInstance>,
    pub coordinator: Option<ProcessInstance>,
    /// `None` when the record was missing or was set aside as unreadable.
    pub record: Option<ParentLossGenerationRecord>,
    pub set_aside: Vec<String>,
    pub notes: Vec<String>,
}

pub(crate) enum ClosureOutcome {
    /// The closure is written into the generation's record.
    Closed,
    Refused(BootstrapRecoveryReason),
}

/// Read a generation's record for closure. A record that cannot be parsed or
/// names another generation is bookkeeping we cannot trust and is set aside
/// beside itself; the admissions directory is the evidence that survives it.
pub(crate) fn load_record_for_closure(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> Result<(Option<ParentLossGenerationRecord>, Vec<String>), ParentLossLedgerError> {
    use solstone_core_journal_io::durability::{
        ArtifactId, DurableRead, read_json_durable_validated,
    };
    let path = ledger.record_path(generation);
    match read_json_durable_validated::<ParentLossGenerationRecord>(
        ArtifactId::ParentLossRecord,
        &path,
        |record| {
            if record.generation == generation {
                Ok(())
            } else {
                Err(format!(
                    "generation mismatch: expected {generation}, found {}",
                    record.generation
                ))
            }
        },
    ) {
        Ok(DurableRead::Present(record)) => Ok((Some(record), Vec::new())),
        Ok(DurableRead::SetAside(aside)) => {
            let aside = aside
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            Ok((None, vec![aside]))
        }
        Ok(DurableRead::Absent | DurableRead::Unreadable { .. }) => Ok((None, Vec::new())),
        Err(error) => Err(error.into()),
    }
}

/// Close one abandoned generation. The caller holds the active-generation lock
/// and the coordinator lease, and its parent holds the supervisor lock.
pub(crate) fn close_abandoned_generation(
    ledger: &ParentLossLedger,
    abandoned: AbandonedGeneration,
    successor_generation: ParentLossGeneration,
    authority: &ClosingAuthority<'_>,
) -> Result<ClosureOutcome, ParentLossLedgerError> {
    let AbandonedGeneration {
        generation,
        supervisor,
        coordinator,
        record,
        set_aside,
        mut notes,
    } = abandoned;

    // 1. The authorities. A `SameLive` observation vetoes the lock proof.
    let supervisor_observation = match supervisor {
        None => AuthorityObservation::NeverRecorded,
        Some(instance) => match observe_settled(authority.source, &instance, authority.deadline) {
            InstanceVerdict::SameLive { .. } => {
                return Ok(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::SupervisorLive,
                ));
            }
            InstanceVerdict::NotSameOrExited => AuthorityObservation::Exited,
            InstanceVerdict::Unverifiable => AuthorityObservation::Unobservable,
        },
    };
    let coordinator_observation = match coordinator {
        None => AuthorityObservation::NeverRecorded,
        Some(instance) => match observe_settled(authority.source, &instance, authority.deadline) {
            InstanceVerdict::SameLive { .. } => {
                return Ok(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::ActiveCoordinator,
                ));
            }
            InstanceVerdict::NotSameOrExited => AuthorityObservation::Exited,
            InstanceVerdict::Unverifiable => AuthorityObservation::Unobservable,
        },
    };

    // 2. Everything the generation admitted. The pointer is marked sealed
    // FIRST, under the active lock the caller holds: a child whose launcher
    // died mid-launch re-checks the phase after it acknowledges, so an
    // acknowledgement that lands after this scan meets a sealed pointer and
    // the child refuses to serve. One that lands before the scan is found
    // here and retired.
    ledger.mark_sealed_for_closure(generation)?;
    let (mut admissions, sealed) = scan_admissions(ledger, generation)?;
    if let Some(reason) = retire_live_admissions(&mut admissions, authority) {
        return Ok(ClosureOutcome::Refused(reason));
    }

    // 3. The dead supervisor's heartbeat, which the coordinator would have
    // cleared on a confirmed parent loss had it lived to see one.
    let heartbeat = match (
        record
            .as_ref()
            .and_then(|record| record.supervisor_heartbeat.clone()),
        supervisor,
    ) {
        (Some(filename), Some(instance)) => clear_supervisor_heartbeat(ledger, &filename, instance),
        _ => HeartbeatRetirement::NotRecorded,
    };

    // 4. The sealed ledger. A coordinator that sealed and then died left one;
    // its bytes are kept and only digested.
    let ledger_path = ledger.sealed_ledger_path(generation);
    let sealed_ledger_digest = match fs::read(&ledger_path) {
        Ok(bytes) => digest_bytes(&bytes),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            ledger.write_sealed_ledger(generation, &sealed)?
        }
        Err(error) => return Err(error.into()),
    };

    // 5. The record: terminal if it had none, plus this closure. Written
    // BEFORE the successor's pointer, so a crash in between leaves a sealed
    // unresolved generation with a dead coordinator, which the ordinary
    // escape already advances past.
    let mut record = record.unwrap_or_else(|| ParentLossGenerationRecord {
        schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
        generation,
        coordinator,
        supervisor: supervisor.unwrap_or(authority.closed_by),
        sealed_ledger_digest: None,
        terminal: None,
        supervisor_heartbeat: None,
        closure: None,
    });
    if supervisor.is_none() {
        notes.push("no supervisor identity was recorded; the record names the closer".to_owned());
    }
    if record.terminal.is_none() {
        record.terminal = Some(ParentLossTerminalDisposition::Unresolved {
            reason: ParentLossUnresolvedReason::AuthoritiesLost,
        });
    }
    if record.sealed_ledger_digest.is_none() {
        record.sealed_ledger_digest = Some(sealed_ledger_digest);
    }
    let closure = AbandonedGenerationClosure {
        schema: PARENT_LOSS_CLOSURE_SCHEMA_V1,
        closed_by: authority.closed_by,
        successor_generation,
        closed_at: unix_seconds(),
        supervisor: supervisor_observation,
        coordinator: coordinator_observation,
        heartbeat,
        set_aside,
        notes,
        admissions,
    };
    record.closure = Some(closure);
    let record_path = ledger.record_path(generation);
    fs::create_dir_all(record_path.parent().expect("generation record parent"))?;
    write_json(&record_path, &record, json_options())?;
    Ok(ClosureOutcome::Closed)
}

/// Walk the admissions directory under the same lock the launchers and the
/// coordinator's seal take, so a launch in flight from a still-exiting service
/// cannot slip past the scan.
fn scan_admissions(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> Result<(Vec<AdmissionClosure>, Vec<SealedAdmission>), ParentLossLedgerError> {
    let _lock = hold_lock(
        ledger.admission_lock_path(generation),
        LockOptions {
            timeout: ADMISSION_LOCK_TIMEOUT,
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )?;
    let directory = admission_directory(ledger, generation);
    let mut closures = Vec::new();
    let mut sealed = Vec::new();
    let entries = match fs::read_dir(&directory) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Ok((closures, sealed));
        }
        Err(error) => return Err(error.into()),
    };
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_dir() {
            continue;
        }
        let launch_id = entry.file_name().to_string_lossy().into_owned();
        let intent = read_parent_loss_admission_intent(ledger, generation, &launch_id)
            .ok()
            .flatten();
        let service = intent.as_ref().and_then(|intent| intent.service);
        let result = read_parent_loss_admission_result(ledger, generation, &launch_id)
            .ok()
            .flatten();
        let acknowledgement =
            read_parent_loss_admission_acknowledgement_in(ledger, generation, &launch_id)
                .ok()
                .flatten();
        let (identity, finding) = match result {
            Some(result) => match result.state {
                AdmissionResultState::Admitted | AdmissionResultState::RejectedUnreaped { .. } => {
                    match result
                        .identity
                        .or_else(|| acknowledgement.map(|ack| ack.identity))
                    {
                        Some(identity) => (Some(identity), None),
                        None => (None, Some(AdmissionFinding::Unidentified)),
                    }
                }
                AdmissionResultState::SpawnFailed { .. } => {
                    (None, Some(AdmissionFinding::SpawnFailed))
                }
                AdmissionResultState::RejectedAndReaped { .. } => {
                    (None, Some(AdmissionFinding::RejectedAndReaped))
                }
            },
            None => match acknowledgement {
                Some(acknowledgement) => (Some(acknowledgement.identity), None),
                None => (None, Some(AdmissionFinding::NeverAcknowledged)),
            },
        };
        if let Some(identity) = identity.as_ref() {
            sealed.push(SealedAdmission {
                launch_id: launch_id.clone(),
                service,
                identity: identity.clone(),
            });
        }
        closures.push(AdmissionClosure {
            launch_id,
            service,
            instance: identity.as_ref().map(|identity| identity.instance),
            uid: identity.as_ref().map(|identity| identity.uid),
            // A placeholder for identified targets; `retire_live_admissions`
            // replaces it with what it observed.
            finding: finding.unwrap_or(AdmissionFinding::Exited),
        });
    }
    sealed.sort_by(|left, right| left.launch_id.cmp(&right.launch_id));
    closures.sort_by(|left, right| left.launch_id.cmp(&right.launch_id));
    Ok((closures, sealed))
}

/// Observe every identified admission; signal the live ones exactly and wait
/// for them within the deadline. Returns the refusal reason if any remain.
fn retire_live_admissions(
    admissions: &mut [AdmissionClosure],
    authority: &ClosingAuthority<'_>,
) -> Option<BootstrapRecoveryReason> {
    let mut pending = Vec::new();
    let mut unverifiable = false;
    for (index, admission) in admissions.iter_mut().enumerate() {
        let Some(instance) = admission.instance else {
            continue;
        };
        match observe_settled(authority.source, &instance, authority.deadline) {
            InstanceVerdict::NotSameOrExited => admission.finding = AdmissionFinding::Exited,
            InstanceVerdict::Unverifiable => {
                match settle_unverifiable(authority, instance.pid, admission.uid) {
                    Some(finding) => admission.finding = finding,
                    None => unverifiable = true,
                }
            }
            InstanceVerdict::SameLive { .. } => {
                // A failed signal here means the exact instance was gone by the
                // time the guard re-observed it; the wait below settles it.
                let _ = authority.retirer.signal(instance, SignalKind::Terminate);
                pending.push((index, instance));
            }
        }
    }
    if unverifiable {
        // ⛔ Not a converging reason by itself, and not read as death: the
        // pid may still be ours and this start cannot prove otherwise.
        return Some(BootstrapRecoveryReason::AbandonedAdmissionUnverifiable);
    }
    if pending.is_empty() {
        return None;
    }
    let kill_at = authority
        .deadline
        .checked_sub(ESCALATION_RESERVE)
        .unwrap_or(authority.deadline)
        .max(Instant::now());
    pending = wait_for_exit(admissions, pending, authority, kill_at, false);
    if pending.is_empty() {
        return None;
    }
    for (_, instance) in &pending {
        let _ = authority.retirer.signal(*instance, SignalKind::Kill);
    }
    let pending = wait_for_exit(admissions, pending, authority, authority.deadline, true);
    if pending.is_empty() {
        None
    } else {
        Some(BootstrapRecoveryReason::AbandonedAdmissionLive)
    }
}

fn wait_for_exit(
    admissions: &mut [AdmissionClosure],
    mut pending: Vec<(usize, ProcessInstance)>,
    authority: &ClosingAuthority<'_>,
    until: Instant,
    escalated: bool,
) -> Vec<(usize, ProcessInstance)> {
    loop {
        pending.retain(
            |(index, instance)| match authority.source.observe(instance) {
                InstanceVerdict::SameLive { .. } => true,
                InstanceVerdict::NotSameOrExited => {
                    admissions[*index].finding = AdmissionFinding::Retired { escalated };
                    false
                }
                InstanceVerdict::Unverifiable => {
                    // Mid-retirement the pid was ours a moment ago; only a
                    // positive owner reading may settle it.
                    match settle_unverifiable(authority, instance.pid, admissions[*index].uid) {
                        Some(finding) => {
                            admissions[*index].finding = finding;
                            false
                        }
                        None => true,
                    }
                }
            },
        );
        if pending.is_empty() || Instant::now() >= until {
            return pending;
        }
        thread::sleep(OBSERVATION_POLL.min(until.saturating_duration_since(Instant::now())));
    }
}

/// What a pid `inspect` cannot read may still be settled by: the process
/// table's owner for that pid. Gone is exited; another user's is a reused
/// pid; our own uid or an instrument failure settles nothing.
fn settle_unverifiable(
    authority: &ClosingAuthority<'_>,
    pid: u32,
    recorded_uid: Option<u32>,
) -> Option<AdmissionFinding> {
    match authority.retirer.owner(pid) {
        ProcessOwner::Absent => Some(AdmissionFinding::Exited),
        ProcessOwner::Uid(uid) if recorded_uid.is_some_and(|recorded| recorded != uid) => {
            Some(AdmissionFinding::PidReusedByAnotherUser)
        }
        ProcessOwner::Uid(_) | ProcessOwner::Unknown => None,
    }
}

/// Re-observe a transient `Unverifiable` for a bounded window, so a pid that
/// exits between two procfs reads is not recorded as unobservable.
fn observe_settled(
    source: &dyn ProcessInstanceSource,
    instance: &ProcessInstance,
    deadline: Instant,
) -> InstanceVerdict {
    let settle = (Instant::now() + OBSERVATION_SETTLE_WINDOW).min(deadline);
    loop {
        let verdict = source.observe(instance);
        if verdict != InstanceVerdict::Unverifiable || Instant::now() >= settle {
            return verdict;
        }
        thread::sleep(OBSERVATION_POLL);
    }
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use std::collections::{HashMap, HashSet};
    use std::path::Path;
    use std::sync::{Arc, Mutex};

    use tempfile::TempDir;

    use super::*;
    use crate::lifecycle::parent_loss_admission::{
        AdmissionIdentity, AdmissionIntent, AdmissionResult, acknowledge_parent_loss_admission,
        write_parent_loss_admission_intent, write_parent_loss_admission_result,
    };
    use crate::lifecycle::parent_loss_ledger::{
        ActiveGeneration, CoordinatorLease, ParentLossReaderOutcome, read_parent_loss_outcome,
    };
    use crate::lifecycle::{HEARTBEAT_SCHEMA_V2, HeartbeatV2, RunId, WriterId};
    use crate::process::{ExecutionState, InspectResult, InstanceCensus, ProcessBirth};

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    /// A process table under test control: a pid is live with one exact
    /// identity, unverifiable, or absent.
    #[derive(Default)]
    struct FakeTable {
        live: HashMap<u32, (ProcessInstance, u32)>,
        unverifiable: HashSet<u32>,
        /// What the process table says owns a pid `inspect` cannot read.
        owners: HashMap<u32, ProcessOwner>,
    }

    #[derive(Clone, Default)]
    struct FakeSource(Arc<Mutex<FakeTable>>);

    impl ProcessInstanceSource for FakeSource {
        fn inspect(&self, pid: u32) -> InspectResult {
            let table = self.0.lock().expect("fake table");
            if table.unverifiable.contains(&pid) {
                return InspectResult::Unverifiable;
            }
            match table.live.get(&pid) {
                Some((instance, uid)) => InspectResult::Present {
                    instance: *instance,
                    uid: *uid,
                    execution: ExecutionState::Running,
                    ppid: Some(1),
                    pgid: Some(1),
                },
                None => InspectResult::Absent,
            }
        }

        fn census(&self) -> InstanceCensus {
            InstanceCensus::Complete(Vec::new())
        }
    }

    /// Records every signal; a process exits on `Terminate` only when it is
    /// in `obeys_terminate`, and on `Kill` unless it is `immortal`.
    struct FakeRetirer {
        table: Arc<Mutex<FakeTable>>,
        signals: Mutex<Vec<(u32, &'static str)>>,
        obeys_terminate: HashSet<u32>,
        immortal: HashSet<u32>,
    }

    impl AdmissionRetirer for FakeRetirer {
        fn signal(
            &self,
            instance: ProcessInstance,
            signal: SignalKind,
        ) -> Result<(), TerminationError> {
            let name = match signal {
                SignalKind::Terminate => "term",
                SignalKind::Kill => "kill",
            };
            self.signals
                .lock()
                .expect("signals")
                .push((instance.pid, name));
            let exits = match signal {
                SignalKind::Terminate => self.obeys_terminate.contains(&instance.pid),
                SignalKind::Kill => !self.immortal.contains(&instance.pid),
            };
            if exits {
                self.table
                    .lock()
                    .expect("fake table")
                    .live
                    .remove(&instance.pid);
            }
            Ok(())
        }

        fn owner(&self, pid: u32) -> ProcessOwner {
            let table = self.table.lock().expect("fake table");
            if let Some(owner) = table.owners.get(&pid) {
                return *owner;
            }
            match table.live.get(&pid) {
                Some((_, uid)) => ProcessOwner::Uid(*uid),
                None => ProcessOwner::Absent,
            }
        }
    }

    fn fake(
        live: &[(ProcessInstance, u32)],
        obeys_terminate: &[u32],
        immortal: &[u32],
    ) -> (FakeSource, FakeRetirer) {
        let table = Arc::new(Mutex::new(FakeTable {
            live: live.iter().map(|(i, uid)| (i.pid, (*i, *uid))).collect(),
            unverifiable: HashSet::new(),
            owners: HashMap::new(),
        }));
        let retirer = FakeRetirer {
            table: Arc::clone(&table),
            signals: Mutex::new(Vec::new()),
            obeys_terminate: obeys_terminate.iter().copied().collect(),
            immortal: immortal.iter().copied().collect(),
        };
        (FakeSource(table), retirer)
    }

    /// An open, admitting generation whose supervisor and coordinator are
    /// synthetic identities nothing on this host can match.
    fn open_generation(directory: &TempDir) -> (ParentLossLedger, ActiveGeneration) {
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [HostedServiceKind::Sense])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let coordinator = instance(20, 2);
        let active = ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        let active = ledger
            .mark_admitting(active.generation, coordinator)
            .expect("admitting state");
        (ledger, active)
    }

    fn admit(
        journal: &Path,
        generation: ParentLossGeneration,
        launch_id: &str,
        service: Option<HostedServiceKind>,
        identity: Option<ProcessInstance>,
    ) -> Option<AdmissionIdentity> {
        write_parent_loss_admission_intent(
            journal,
            &AdmissionIntent::new(generation, launch_id, service, None),
        )
        .expect("intent");
        let identity = identity.map(|instance| AdmissionIdentity {
            generation,
            launch_id: launch_id.to_owned(),
            instance,
            uid: 501,
            parent_launch_id: None,
        });
        if let Some(identity) = identity.as_ref() {
            acknowledge_parent_loss_admission(journal, identity.clone()).expect("ack");
            write_parent_loss_admission_result(
                journal,
                generation,
                launch_id,
                &AdmissionResult {
                    schema: 1,
                    identity: Some(identity.clone()),
                    state: AdmissionResultState::Admitted,
                },
            )
            .expect("result");
        }
        identity
    }

    fn lease(ledger: &ParentLossLedger) -> CoordinatorLease {
        ledger
            .acquire_coordinator_lease()
            .expect("lease acquisition")
            .expect("lease free")
    }

    fn authority<'a>(
        source: &'a dyn ProcessInstanceSource,
        retirer: &'a dyn AdmissionRetirer,
        budget: Duration,
    ) -> ClosingAuthority<'a> {
        ClosingAuthority {
            closed_by: instance(30, 3),
            source,
            retirer,
            deadline: Instant::now() + budget,
        }
    }

    fn closure_of(
        ledger: &ParentLossLedger,
        generation: ParentLossGeneration,
    ) -> AbandonedGenerationClosure {
        ledger
            .record(generation)
            .expect("record readable")
            .expect("record exists")
            .closure
            .expect("closure recorded")
    }

    #[test]
    fn an_open_generation_whose_authorities_are_gone_is_closed_and_the_successor_boots() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let exited = instance(4242, 42);
        admit(
            directory.path(),
            active.generation,
            "sense-a",
            Some(HostedServiceKind::Sense),
            Some(exited),
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);

        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("an abandoned generation must not brick the journal");

        assert_eq!(successor.generation, active.generation + 1);
        let record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record kept");
        assert_eq!(
            record.terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AuthoritiesLost,
            })
        );
        let closure = record.closure.expect("closure recorded");
        assert_eq!(closure.successor_generation, successor.generation);
        assert_eq!(closure.supervisor, AuthorityObservation::Exited);
        assert_eq!(closure.coordinator, AuthorityObservation::Exited);
        assert_eq!(closure.admissions.len(), 1);
        assert_eq!(closure.admissions[0].finding, AdmissionFinding::Exited);
        assert!(retirer.signals.lock().expect("signals").is_empty());
        assert!(
            ledger.sealed_ledger_path(active.generation).is_file(),
            "the closed generation carries a sealed ledger"
        );
        // The successor now reads as the ordinary reservation seam.
        assert!(matches!(
            read_parent_loss_outcome(directory.path()).expect("outcome"),
            ParentLossReaderOutcome::BootstrapRecoveryRequired { .. }
        ));
    }

    /// 🔒 The lock is the proof and the observation is the veto: a coordinator
    /// that is genuinely live -- this test process -- refuses the closer even
    /// though the test holds the lease.
    #[test]
    fn a_live_coordinator_still_refuses_the_closer() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        let live = match SystemProcessInstanceSource.inspect(std::process::id()) {
            InspectResult::Present { instance, .. } => instance,
            _ => panic!("this test process must be observable"),
        };
        ledger
            .persist_coordinator_identity(active.generation, live)
            .expect("coordinator identity");
        let lease = lease(&ledger);

        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(
                &SystemProcessInstanceSource,
                &SystemAdmissionRetirer,
                Duration::from_secs(2),
            ),
        );

        assert!(matches!(
            result,
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::ActiveCoordinator
            ))
        ));
        assert!(
            ledger
                .record(active.generation)
                .expect("record")
                .expect("record")
                .closure
                .is_none(),
            "a refused closure writes nothing"
        );
    }

    #[test]
    fn a_live_supervisor_still_refuses_the_closer() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let live = match SystemProcessInstanceSource.inspect(std::process::id()) {
            InspectResult::Present { instance, .. } => instance,
            _ => panic!("this test process must be observable"),
        };
        let active = ledger
            .reserve_generation(live, [])
            .expect("reserve generation");
        ledger.initialize_record(&active).expect("record");
        ledger
            .persist_coordinator_identity(active.generation, instance(20, 2))
            .expect("coordinator identity");
        let lease = lease(&ledger);

        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(
                &SystemProcessInstanceSource,
                &SystemAdmissionRetirer,
                Duration::from_secs(2),
            ),
        );

        assert!(matches!(
            result,
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::SupervisorLive
            ))
        ));
    }

    /// 🔒 Safety over convergence: a process that survives SIGKILL within the
    /// deadline keeps this start refused, the generation open, and nothing
    /// written. The next start retries.
    #[test]
    fn an_admission_that_outlives_the_deadline_refuses_this_start_and_leaves_the_generation_open() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let immortal = instance(5151, 51);
        admit(
            directory.path(),
            active.generation,
            "sense-immortal",
            Some(HostedServiceKind::Sense),
            Some(immortal),
        );
        let (source, retirer) = fake(&[(immortal, 501)], &[], &[immortal.pid]);
        let lease = lease(&ledger);

        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_millis(400)),
        );

        assert!(matches!(
            result,
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionLive
            ))
        ));
        let signals = retirer.signals.lock().expect("signals").clone();
        assert_eq!(
            signals,
            vec![(immortal.pid, "term"), (immortal.pid, "kill")]
        );
        let record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record");
        assert!(record.terminal.is_none(), "the generation stays open");
        assert!(record.closure.is_none());
        assert_eq!(
            ledger
                .active_generation()
                .expect("pointer")
                .expect("pointer kept")
                .generation,
            active.generation
        );
    }

    #[test]
    fn a_child_that_obeys_sigterm_is_not_escalated_and_the_signals_are_recorded() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let polite = instance(6161, 61);
        admit(
            directory.path(),
            active.generation,
            "spl-polite",
            Some(HostedServiceKind::Spl),
            Some(polite),
        );
        let (source, retirer) = fake(&[(polite, 501)], &[polite.pid], &[]);
        let lease = lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        assert_eq!(
            retirer.signals.lock().expect("signals").clone(),
            vec![(polite.pid, "term")]
        );
        assert_eq!(
            closure_of(&ledger, active.generation).admissions[0].finding,
            AdmissionFinding::Retired { escalated: false }
        );
    }

    /// 🔒 A pid `inspect` cannot read is settled only by a positive owner
    /// reading: another user's pid is a reused pid; our own uid, or no
    /// answer, refuses this start rather than reading "cannot tell" as death.
    #[test]
    fn an_uninspectable_pid_is_settled_by_its_owner_or_refuses() {
        for (owner, expected) in [
            (
                ProcessOwner::Uid(0),
                Ok(AdmissionFinding::PidReusedByAnotherUser),
            ),
            (ProcessOwner::Absent, Ok(AdmissionFinding::Exited)),
            (
                ProcessOwner::Uid(501),
                Err(BootstrapRecoveryReason::AbandonedAdmissionUnverifiable),
            ),
            (
                ProcessOwner::Unknown,
                Err(BootstrapRecoveryReason::AbandonedAdmissionUnverifiable),
            ),
        ] {
            let directory = TempDir::new().expect("temporary root");
            let (ledger, active) = open_generation(&directory);
            let foreign = instance(7171, 71);
            admit(
                directory.path(),
                active.generation,
                "sense-foreign",
                Some(HostedServiceKind::Sense),
                Some(foreign),
            );
            let (source, retirer) = fake(&[], &[], &[]);
            {
                let mut table = source.0.lock().expect("fake table");
                table.unverifiable.insert(foreign.pid);
                table.owners.insert(foreign.pid, owner);
            }
            let lease = lease(&ledger);

            let result = ledger.reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_millis(600)),
            );

            assert!(retirer.signals.lock().expect("signals").is_empty());
            match expected {
                Ok(finding) => {
                    result.expect("settled by a positive owner reading");
                    assert_eq!(
                        closure_of(&ledger, active.generation).admissions[0].finding,
                        finding,
                        "{owner:?}"
                    );
                }
                Err(reason) => {
                    assert!(
                        matches!(result, Err(ParentLossLedgerError::RecoveryRequired(r)) if r == reason),
                        "{owner:?} must refuse"
                    );
                    assert!(
                        ledger
                            .record(active.generation)
                            .expect("record")
                            .expect("record")
                            .closure
                            .is_none()
                    );
                }
            }
        }
    }

    /// The seal lands before the scan, so an acknowledgement written after the
    /// scan meets a sealed pointer.
    #[test]
    fn the_pointer_is_sealed_before_the_admissions_are_scanned() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);
        let closing = ClosingAuthority {
            closed_by: instance(30, 3),
            source: &source,
            retirer: &retirer,
            deadline: Instant::now() + Duration::from_secs(2),
        };
        let abandoned = AbandonedGeneration {
            generation: active.generation,
            supervisor: Some(active.supervisor),
            coordinator: active.coordinator,
            record: ledger.record(active.generation).expect("record"),
            set_aside: Vec::new(),
            notes: Vec::new(),
        };
        let _ = lease;
        assert!(matches!(
            close_abandoned_generation(&ledger, abandoned, active.generation + 1, &closing)
                .expect("closed"),
            ClosureOutcome::Closed
        ));
        assert_eq!(
            ledger
                .active_generation()
                .expect("pointer")
                .expect("pointer")
                .phase,
            crate::lifecycle::ParentLossPhase::Sealed
        );
    }

    #[test]
    fn an_intent_without_acknowledgement_is_recorded_never_acknowledged() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "sense-unacked",
            Some(HostedServiceKind::Sense),
            None,
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        let closure = closure_of(&ledger, active.generation);
        assert_eq!(
            closure.admissions[0].finding,
            AdmissionFinding::NeverAcknowledged
        );
        assert!(closure.admissions[0].instance.is_none());
    }

    #[test]
    fn a_missing_record_is_created_and_closed() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reservation without a record");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);

        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("a reservation that died before its record must not brick");

        assert_eq!(successor.generation, active.generation + 1);
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.coordinator, AuthorityObservation::NeverRecorded);
        assert_eq!(closure.supervisor, AuthorityObservation::Exited);
    }

    #[test]
    fn a_malformed_record_is_set_aside_and_closed() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let live = instance(8181, 81);
        admit(
            directory.path(),
            active.generation,
            "sense-live",
            Some(HostedServiceKind::Sense),
            Some(live),
        );
        std::fs::write(ledger.record_path(active.generation), b"{ not json")
            .expect("corrupt the record");
        let (source, retirer) = fake(&[(live, 501)], &[live.pid], &[]);
        let lease = lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("a malformed record must not brick");

        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.set_aside.len(), 1);
        assert!(closure.set_aside[0].starts_with("record.wedged-"));
        assert!(
            ledger
                .generation_path(active.generation)
                .join(&closure.set_aside[0])
                .is_file(),
            "the damaged record is preserved beside the new one"
        );
        assert_eq!(
            closure.admissions[0].finding,
            AdmissionFinding::Retired { escalated: false },
            "the admissions directory outlives the record and still retires"
        );
    }

    #[test]
    fn a_set_aside_pointer_closes_the_open_generation_on_disk() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let live = instance(9191, 91);
        admit(
            directory.path(),
            active.generation,
            "cortex-live",
            Some(HostedServiceKind::Cortex),
            Some(live),
        );
        std::fs::write(ledger.active_path(), b"{ this is not json").expect("corrupt the pointer");
        let (source, retirer) = fake(&[(live, 501)], &[live.pid], &[]);
        let lease = lease(&ledger);

        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("a corrupt pointer must not brick");

        assert!(successor.generation > active.generation);
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(
            closure.admissions[0].finding,
            AdmissionFinding::Retired { escalated: false },
            "the generation the pointer named is still retired from disk"
        );
    }

    #[test]
    fn the_dead_supervisors_heartbeat_is_retired_with_the_generation() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let supervisor = instance(10, 1);
        let active = ledger
            .reserve_generation(supervisor, [])
            .expect("reserve generation");
        let writer = WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer id");
        let run = RunId::generate().expect("run id");
        let filename = crate::lifecycle::v2_heartbeat_filename(&writer, &run);
        ledger
            .initialize_record_with_heartbeat(&active, Some(filename.clone()))
            .expect("record");
        let coordinator = instance(20, 2);
        ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .expect("coordinator identity");
        let sync = directory.path().join("health/sync");
        std::fs::create_dir_all(&sync).expect("sync directory");
        let heartbeat = HeartbeatV2::new(
            writer,
            run,
            "host".to_owned(),
            supervisor.pid,
            "1.0".to_owned(),
            "test".to_owned(),
            15,
            directory.path().display().to_string(),
        );
        assert_eq!(heartbeat.schema, HEARTBEAT_SCHEMA_V2);
        std::fs::write(
            sync.join(&filename),
            serde_json::to_vec(&heartbeat).expect("heartbeat json"),
        )
        .expect("heartbeat file");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        assert_eq!(
            closure_of(&ledger, active.generation).heartbeat,
            HeartbeatRetirement::Removed
        );
        assert!(
            !sync.join(&filename).exists(),
            "the dead run's heartbeat is gone"
        );
    }

    /// A record written before the heartbeat field existed says so, and the
    /// stale-heartbeat collector keeps its job.
    #[test]
    fn a_record_without_a_heartbeat_name_records_that_nothing_was_retired() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);
        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");
        assert_eq!(
            closure_of(&ledger, active.generation).heartbeat,
            HeartbeatRetirement::NotRecorded
        );
    }

    /// A sealed `unresolved` generation used to be advanced past with nothing
    /// retired. It is still advanced past, and now its admissions are retired
    /// and its record carries the closure; the terminal disposition is kept.
    #[test]
    fn a_sealed_unresolved_generation_is_advanced_past_with_its_admissions_retired() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let coordinator = active.coordinator.expect("coordinator");
        let lingering = instance(3131, 31);
        admit(
            directory.path(),
            active.generation,
            "sense-lingering",
            Some(HostedServiceKind::Sense),
            Some(lingering),
        );
        ledger.seal(active.generation, coordinator).expect("seal");
        let digest = ledger
            .write_sealed_ledger(active.generation, &Vec::<String>::new())
            .expect("sealed ledger");
        ledger
            .write_terminal_with_digest(
                active.generation,
                coordinator,
                ParentLossTerminalDisposition::Unresolved {
                    reason: ParentLossUnresolvedReason::RetirementDeadlineExceeded {
                        deadline_seconds: 15,
                    },
                },
                Some(digest),
            )
            .expect("terminal");
        let (source, retirer) = fake(&[(lingering, 501)], &[lingering.pid], &[]);
        let lease = lease(&ledger);

        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("advanced");

        assert_eq!(successor.generation, active.generation + 1);
        let record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record");
        assert!(matches!(
            record.terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::RetirementDeadlineExceeded { .. }
            })
        ));
        let closure = record.closure.expect("closure");
        assert_eq!(
            closure.admissions[0].finding,
            AdmissionFinding::Retired { escalated: false }
        );
        assert!(
            closure
                .notes
                .iter()
                .any(|note| note.contains("sealed unresolved"))
        );
    }

    #[test]
    fn a_completed_predecessor_is_not_touched_by_the_closer() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let coordinator = active.coordinator.expect("coordinator");
        ledger.seal(active.generation, coordinator).expect("seal");
        let digest = ledger
            .write_sealed_ledger(active.generation, &Vec::<String>::new())
            .expect("sealed ledger");
        ledger
            .write_terminal_with_digest(
                active.generation,
                coordinator,
                ParentLossTerminalDisposition::CancelledBeforeAdmission,
                Some(digest),
            )
            .expect("terminal");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("advanced");

        assert!(
            ledger
                .record(active.generation)
                .expect("record")
                .expect("record")
                .closure
                .is_none()
        );
    }

    /// The lease is a real flock: a second holder cannot take it while the
    /// first is alive, and the ledger reports that as contention, not error.
    #[test]
    fn the_coordinator_lease_is_exclusive_while_held() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let held = lease(&ledger);
        assert!(
            ledger
                .acquire_coordinator_lease()
                .expect("acquisition")
                .is_none()
        );
        drop(held);
        assert!(
            ledger
                .acquire_coordinator_lease()
                .expect("acquisition")
                .is_some()
        );
    }
}
