// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Closing a generation whose supervisor and coordinator both exited before
//! the coordinator could publish a terminal result.
//!
//! Durability is established through discrete, immutable, whole-JSON artifacts
//! under `health/parent-loss/generations/<gen>/closure-recovery/`:
//! - `batches/<seq:06>.json`: lists completed rows up to `RECOVERY_BATCH_LIMIT` (256).
//! - `ids/<seq:06>.json`: lists launch IDs certified by that batch.
//! - `live/<launch_id>.json`: tracks live process state, census of descendants,
//!   escalate_at, reap deadline, term_requested, and binding digest.
//! - `provisional/<launch_id>.json`: tracks provisional findings (never-acknowledged,
//!   unidentified) awaiting possible late ack/result or finalization.
//! - `phase/scan-complete.json`: written when directory scan is finished, all
//!   provisional drops rechecked, and no live files remain.
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

#![allow(clippy::too_many_arguments, clippy::collapsible_if)]

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};
use solstone_core_journal_io::durability::{
    ArtifactId, DurableRead, read_json_durable, read_json_durable_validated, set_aside,
};
use solstone_core_journal_io::{LockOptions, hold_lock, write_json};

use super::HostedServiceKind;
use super::parent_loss_admission::{
    AdmissionAcknowledgement, AdmissionIdentity, AdmissionIntent, AdmissionResult,
    AdmissionResultState, AdmissionSpawnIdentity, admission_directory, validate_launch_id,
};
use super::parent_loss_coordinator::{
    PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE, PARENT_LOSS_COORDINATOR_TERMINAL_WRITE_BUDGET,
    SealedAdmission, clear_supervisor_heartbeat,
};
use super::parent_loss_ledger::{
    BootstrapRecoveryReason, PARENT_LOSS_LEDGER_SCHEMA_V1, ParentLossGeneration,
    ParentLossGenerationRecord, ParentLossLedger, ParentLossLedgerError, ParentLossPhase,
    ParentLossTerminalDisposition, ParentLossUnresolvedReason, canonical_json, digest_bytes,
    json_options,
};
#[cfg(target_os = "linux")]
use crate::process::InspectResult;
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process::exact_descendant_tree;
use crate::process::{
    InstanceVerdict, ProcessInstance, ProcessInstanceSource, ProcessOwner, SignalKind,
    SystemProcessInstanceSource, TerminationError, process_owner, signal_exact_instance,
};

#[path = "parent_loss_closure_publication.rs"]
mod publication;

#[cfg(test)]
fn inject_finalization_ack(
    ledger: &ParentLossLedger,
    fault: &'static std::thread::LocalKey<std::cell::RefCell<Option<AdmissionIdentity>>>,
) {
    if let Some(identity) = fault.with(|value| value.borrow_mut().take()) {
        super::parent_loss_admission::acknowledge_parent_loss_admission(
            ledger.canonical_root(),
            identity,
        )
        .expect("injected late acknowledgement");
    }
}

pub const PARENT_LOSS_CLOSURE_SCHEMA_V1: u32 = 1;
pub(crate) const PARENT_LOSS_RECOVERY_SCHEMA_V1: u32 = 1;
pub(crate) const RECOVERY_BATCH_LIMIT: usize = 256;
const OBSERVATION_SETTLE_WINDOW: Duration = Duration::from_millis(500);
const OBSERVATION_POLL: Duration = Duration::from_millis(25);
const ADMISSION_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
#[allow(dead_code)]
const ESCALATION_RESERVE: Duration = Duration::from_millis(1_500);
const LIVE_ESCALATION_GRACE_MS: u64 = 12_500;
const LIVE_REAP_GRACE_MS: u64 = 500;

#[cfg(test)]
thread_local! {
    pub(crate) static LEGACY_STOP_AFTER_SEGMENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static LEGACY_DECODED_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static LEGACY_PAYLOAD_WRITE_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static FINALIZATION_ACK_FAULT: std::cell::RefCell<Option<AdmissionIdentity>> = const { std::cell::RefCell::new(None) };
    pub(crate) static FINISH_ACK_FAULT: std::cell::RefCell<Option<AdmissionIdentity>> = const { std::cell::RefCell::new(None) };
    pub(crate) static ASSEMBLY_SEGMENT_LIMIT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static ASSEMBLY_WRITE_FAULT: std::cell::Cell<u8> = const { std::cell::Cell::new(0) };
    pub(crate) static ASSEMBLY_SPOOL_READ_BYTES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static LEGACY_LEDGER_MUTATION_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static ASSEMBLY_STOP_AFTER_SEGMENT: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static ASSEMBLY_CONSUMED_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static HISTORICAL_DISCOVERIES: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static DISCOVERY_STOP_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static SCAN_PUBLICATIONS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static AUDIT_BATCH_READS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static AUDIT_VALIDATED_ROWS: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static AUDIT_STOP_AFTER_BATCH: std::cell::Cell<usize> = const { std::cell::Cell::new(0) };
    pub(crate) static PUBLICATION_STOP_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
    pub(crate) static PROVISIONAL_STOP_FAULT: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryArtifactKind {
    Batch,
    Ids,
    Live,
    Provisional,
    ScanComplete,
    Receipt,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryBatchReceipt {
    pub(crate) schema: u32,
    pub(crate) kind: RecoveryArtifactKind,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) seq: u64,
    pub(crate) batch_bytes_digest: String,
    pub(crate) ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct AdmissionSourceSnapshot {
    intent: Option<String>,
    result: Option<String>,
    acknowledgement: Option<String>,
    spawn_identity: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment_identity: Option<AdmissionIdentity>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    environment_journal: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CompletedBatchRow {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sources: Option<AdmissionSourceSnapshot>,
    pub(crate) launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) service: Option<HostedServiceKind>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) instance: Option<ProcessInstance>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) uid: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_launch_id: Option<String>,
    pub(crate) finding: AdmissionFinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parsed_result_state: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) intent_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ack_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spawn_identity_digest: Option<String>,
    pub(crate) binding_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryBatchFile {
    pub(crate) schema: u32,
    pub(crate) kind: RecoveryArtifactKind,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) seq: u64,
    pub(crate) entries: Vec<CompletedBatchRow>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct RecoveryIdsFile {
    pub(crate) schema: u32,
    pub(crate) kind: RecoveryArtifactKind,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) seq: u64,
    pub(crate) ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveDescendantIdentity {
    pub(crate) instance: ProcessInstance,
    pub(crate) uid: u32,
    pub(crate) ppid: u32,
    pub(crate) pgid: u32,
    #[serde(default)]
    pub(crate) escalate_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reap_deadline_unix_ms: Option<u64>,
    #[serde(default)]
    pub(crate) term_requested: bool,
    #[serde(default)]
    pub(crate) escalated: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CensusObligationFile {
    pub(crate) schema: u32,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) obligations: HashMap<String, LiveRecoveryFile>,
    pub(crate) binding_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct CensusRefusalFile {
    pub(crate) schema: u32,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) reason: String,
}

pub(crate) fn compute_census_obligation_digest(
    obligations: &HashMap<String, LiveRecoveryFile>,
) -> Result<String, ParentLossLedgerError> {
    let mut entries: Vec<(&String, &LiveRecoveryFile)> = obligations.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    let bytes = canonical_json(&entries)?;
    Ok(digest_bytes(&bytes))
}

fn has_census_refusal_or_wedged(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> bool {
    if ledger.census_refusal_path(generation).is_file() {
        return true;
    }
    let gen_dir = ledger.generation_path(generation);
    if let Ok(entries) = fs::read_dir(&gen_dir) {
        for entry in entries.flatten() {
            let name = entry.file_name();
            let s = name.to_string_lossy();
            if s.starts_with("census-obligation.wedged-") || s.starts_with("census-refusal.wedged-")
            {
                return true;
            }
        }
    }
    false
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct LiveRecoveryFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) sources: Option<AdmissionSourceSnapshot>,
    pub(crate) schema: u32,
    pub(crate) kind: RecoveryArtifactKind,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) service: Option<HostedServiceKind>,
    pub(crate) root: ProcessInstance,
    pub(crate) uid: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) parent_launch_id: Option<String>,
    pub(crate) descendants: Vec<LiveDescendantIdentity>,
    pub(crate) escalate_at_unix_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) reap_deadline_unix_ms: Option<u64>,
    pub(crate) term_requested: bool,
    #[serde(default)]
    pub(crate) escalated: bool,
    pub(crate) binding_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ProvisionalFile {
    pub(crate) schema: u32,
    pub(crate) kind: RecoveryArtifactKind,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) launch_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) service: Option<HostedServiceKind>,
    pub(crate) finding: AdmissionFinding,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) intent_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) result_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) ack_digest: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub(crate) spawn_identity_digest: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ScanCompleteFile {
    pub(crate) schema: u32,
    pub(crate) kind: RecoveryArtifactKind,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) sorted_launch_ids: Vec<String>,
    pub(crate) ids_sequences: Vec<u64>,
    pub(crate) ids_digest: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub(crate) struct AdmissionDropDigests {
    pub(crate) intent: Option<String>,
    pub(crate) result: Option<String>,
    pub(crate) ack: Option<String>,
    pub(crate) spawn_identity: Option<String>,
}

impl AdmissionDropDigests {
    #[allow(dead_code)]
    pub(crate) fn is_all_absent(&self) -> bool {
        self.intent.is_none()
            && self.result.is_none()
            && self.ack.is_none()
            && self.spawn_identity.is_none()
    }
}

#[derive(Debug, Clone)]
pub(crate) struct DerivedAdmissionDrops {
    pub(crate) sources: AdmissionSourceSnapshot,
    pub(crate) service: Option<HostedServiceKind>,
    pub(crate) instance: Option<ProcessInstance>,
    pub(crate) uid: Option<u32>,
    pub(crate) parent_launch_id: Option<String>,
    pub(crate) finding: AdmissionFinding,
    pub(crate) parsed_result_state: Option<String>,
    pub(crate) digests: AdmissionDropDigests,
}

#[derive(Serialize)]
struct CanonicalCompletedBinding<'a> {
    launch_id: &'a str,
    service: Option<HostedServiceKind>,
    instance: Option<ProcessInstance>,
    uid: Option<u32>,
    parent_launch_id: Option<&'a str>,
    finding: AdmissionFinding,
    parsed_result_state: Option<&'a str>,
    intent_digest: Option<&'a str>,
    result_digest: Option<&'a str>,
    ack_digest: Option<&'a str>,
    spawn_identity_digest: Option<&'a str>,
}

pub(crate) fn compute_batch_row_binding_digest(
    launch_id: &str,
    service: Option<HostedServiceKind>,
    instance: Option<ProcessInstance>,
    uid: Option<u32>,
    parent_launch_id: Option<&str>,
    finding: AdmissionFinding,
    parsed_result_state: Option<&str>,
    intent_digest: Option<&str>,
    result_digest: Option<&str>,
    ack_digest: Option<&str>,
    spawn_identity_digest: Option<&str>,
) -> Result<String, ParentLossLedgerError> {
    let binding = CanonicalCompletedBinding {
        launch_id,
        service,
        instance,
        uid,
        parent_launch_id,
        finding,
        parsed_result_state,
        intent_digest,
        result_digest,
        ack_digest,
        spawn_identity_digest,
    };
    let bytes = canonical_json(&binding)?;
    Ok(digest_bytes(&bytes))
}

#[derive(Serialize)]
struct CanonicalLiveBinding<'a> {
    root: ProcessInstance,
    uid: u32,
    descendants: &'a [LiveDescendantIdentity],
}

pub(crate) fn compute_live_binding_digest(
    root: ProcessInstance,
    uid: u32,
    descendants: &[LiveDescendantIdentity],
) -> Result<String, ParentLossLedgerError> {
    let binding = CanonicalLiveBinding {
        root,
        uid,
        descendants,
    };
    let bytes = canonical_json(&binding)?;
    Ok(digest_bytes(&bytes))
}

pub(crate) fn completed_row_is_authorized(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    row: &CompletedBatchRow,
) -> bool {
    if validate_launch_id(&row.launch_id).is_err() {
        return false;
    }
    match row.finding {
        AdmissionFinding::NeverAcknowledged | AdmissionFinding::Unidentified => return false,
        AdmissionFinding::Exited
        | AdmissionFinding::Retired { .. }
        | AdmissionFinding::PidReusedByAnotherUser => {
            if row.instance.is_none() || row.uid.is_none() {
                return false;
            }
        }
        AdmissionFinding::SpawnFailed => {
            if row.parsed_result_state.as_deref() != Some("spawn_failed")
                || row.result_digest.is_none()
            {
                return false;
            }
        }
        AdmissionFinding::RejectedAndReaped => {
            if row.parsed_result_state.as_deref() != Some("rejected_and_reaped")
                || row.result_digest.is_none()
            {
                return false;
            }
        }
    }
    let Ok(expected_digest) = compute_batch_row_binding_digest(
        &row.launch_id,
        row.service,
        row.instance,
        row.uid,
        row.parent_launch_id.as_deref(),
        row.finding,
        row.parsed_result_state.as_deref(),
        row.intent_digest.as_deref(),
        row.result_digest.as_deref(),
        row.ack_digest.as_deref(),
        row.spawn_identity_digest.as_deref(),
    ) else {
        return false;
    };
    if row.binding_digest != expected_digest {
        return false;
    }

    let derived = match &row.sources {
        Some(sources)
            if sources.environment_identity.is_some()
                && sources.environment_journal.as_deref() != ledger.canonical_root().to_str() =>
        {
            return false;
        }
        Some(sources) => derive_admission_source_facts(generation, &row.launch_id, sources),
        None => read_derived_admission_drops(ledger, generation, &row.launch_id),
    };
    let Ok(derived) = derived else {
        return false;
    };

    if row.service != derived.service
        || row.parent_launch_id != derived.parent_launch_id
        || row.intent_digest != derived.digests.intent
        || row.result_digest != derived.digests.result
        || row.ack_digest != derived.digests.ack
        || row.spawn_identity_digest != derived.digests.spawn_identity
    {
        return false;
    }

    match row.finding {
        AdmissionFinding::Exited
        | AdmissionFinding::Retired { .. }
        | AdmissionFinding::PidReusedByAnotherUser => {
            if row.instance != derived.instance
                || row.uid != derived.uid
                || row.parsed_result_state != derived.parsed_result_state
            {
                return false;
            }
        }
        AdmissionFinding::SpawnFailed => {
            if row.instance.is_some()
                || row.uid.is_some()
                || derived.parsed_result_state.as_deref() != Some("spawn_failed")
                || derived.digests.result.is_none()
            {
                return false;
            }
        }
        AdmissionFinding::RejectedAndReaped => {
            if row.instance.is_some()
                || row.uid.is_some()
                || derived.parsed_result_state.as_deref() != Some("rejected_and_reaped")
                || derived.digests.result.is_none()
            {
                return false;
            }
        }
        AdmissionFinding::NeverAcknowledged | AdmissionFinding::Unidentified => return false,
    }

    true
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

/// Compute grace duration as:
/// PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE - PARENT_LOSS_COORDINATOR_TERMINAL_WRITE_BUDGET - ESCALATION_RESERVE.
#[allow(dead_code)]
pub(crate) fn grace_duration() -> Duration {
    PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE
        .saturating_sub(PARENT_LOSS_COORDINATOR_TERMINAL_WRITE_BUDGET)
        .saturating_sub(ESCALATION_RESERVE)
}

pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

/// Every drop is created exclusively and then filled, so a writer killed
/// between the two leaves an empty file under the final name. No complete drop
/// is empty: read one as the drop that was never written, rather than as
/// unverifiable evidence that refuses every later start. That is the state a
/// kill just before the create leaves, so every closure path already settles
/// it; a child still writing its acknowledgement re-checks admission after the
/// write and finds the generation sealed before this scan.
fn read_drop_bytes(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) if bytes.is_empty() => Ok(None),
        Ok(bytes) => Ok(Some(bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

pub(crate) fn read_derived_admission_drops(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<DerivedAdmissionDrops, ParentLossLedgerError> {
    if validate_launch_id(launch_id).is_err() {
        return Err(ParentLossLedgerError::RecoveryRequired(
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
        ));
    }
    let admission_dir = admission_directory(ledger, generation).join(launch_id);
    let intent_bytes = read_drop_bytes(&admission_dir.join("intent.json"))?;
    let result_bytes = read_drop_bytes(&admission_dir.join("result.json"))?;
    let ack_bytes = read_drop_bytes(&admission_dir.join("acknowledgement.json"))?;
    let spawn_bytes = read_drop_bytes(&admission_dir.join("spawn-identity.json"))?;

    let to_text = |bytes: Option<Vec<u8>>| -> Result<Option<String>, ParentLossLedgerError> {
        bytes
            .map(String::from_utf8)
            .transpose()
            .map_err(|_| unverifiable_admission())
    };
    let sources = AdmissionSourceSnapshot {
        intent: to_text(intent_bytes)?,
        result: to_text(result_bytes)?,
        acknowledgement: to_text(ack_bytes)?,
        spawn_identity: to_text(spawn_bytes)?,
        environment_identity: None,
        environment_journal: None,
    };
    let derived = derive_admission_source_facts(generation, launch_id, &sources)?;
    #[cfg(target_os = "linux")]
    if derived.finding == AdmissionFinding::NeverAcknowledged
        && let Some(identity) =
            find_live_process_by_launch_env(ledger.canonical_root(), generation, launch_id)
    {
        let mut observed_sources = sources;
        observed_sources.environment_identity = Some(identity);
        observed_sources.environment_journal = ledger.canonical_root().to_str().map(str::to_owned);
        return derive_admission_source_facts(generation, launch_id, &observed_sources);
    }
    Ok(derived)
}

fn unverifiable_admission() -> ParentLossLedgerError {
    ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::AbandonedAdmissionUnverifiable)
}

fn derive_admission_source_facts(
    generation: ParentLossGeneration,
    launch_id: &str,
    sources: &AdmissionSourceSnapshot,
) -> Result<DerivedAdmissionDrops, ParentLossLedgerError> {
    validate_launch_id(launch_id).map_err(|_| unverifiable_admission())?;
    let intent_bytes = sources.intent.as_ref().map(|s| s.as_bytes());
    let result_bytes = sources.result.as_ref().map(|s| s.as_bytes());
    let ack_bytes = sources.acknowledgement.as_ref().map(|s| s.as_bytes());
    let spawn_bytes = sources.spawn_identity.as_ref().map(|s| s.as_bytes());
    let digests = AdmissionDropDigests {
        intent: intent_bytes.as_ref().map(|b| digest_bytes(b)),
        result: result_bytes.as_ref().map(|b| digest_bytes(b)),
        ack: ack_bytes.as_ref().map(|b| digest_bytes(b)),
        spawn_identity: spawn_bytes.as_ref().map(|b| digest_bytes(b)),
    };

    let intent: Option<AdmissionIntent> = intent_bytes
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| unverifiable_admission())?;
    let result: Option<AdmissionResult> = result_bytes
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| unverifiable_admission())?;
    let acknowledgement: Option<AdmissionAcknowledgement> = ack_bytes
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| unverifiable_admission())?;
    let spawn_identity: Option<AdmissionSpawnIdentity> = spawn_bytes
        .map(serde_json::from_slice)
        .transpose()
        .map_err(|_| unverifiable_admission())?;

    if intent.as_ref().is_some_and(|v| v.schema != 1)
        || result.as_ref().is_some_and(|v| v.schema != 1)
        || acknowledgement.as_ref().is_some_and(|v| v.schema != 1)
        || spawn_identity.as_ref().is_some_and(|v| v.schema != 1)
    {
        return Err(unverifiable_admission());
    }
    let identities: Vec<&AdmissionIdentity> = [
        result.as_ref().and_then(|v| v.identity.as_ref()),
        acknowledgement.as_ref().map(|v| &v.identity),
        spawn_identity.as_ref().map(|v| &v.identity),
    ]
    .into_iter()
    .flatten()
    .collect();
    if identities.windows(2).any(|v| v[0] != v[1])
        || identities.iter().any(|id| {
            intent
                .as_ref()
                .is_some_and(|i| i.parent_launch_id != id.parent_launch_id)
        })
    {
        return Err(unverifiable_admission());
    }
    if sources.environment_identity.is_some() != sources.environment_journal.is_some() {
        return Err(unverifiable_admission());
    }
    if let Some(id) = &sources.environment_identity {
        if id.parent_launch_id.is_some()
            || !identities.is_empty()
            || result.is_some()
            || id.generation != generation
            || id.launch_id != launch_id
        {
            return Err(unverifiable_admission());
        }
    }
    if let Some(i) = &intent {
        if i.generation != generation || i.launch_id != launch_id {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
            ));
        }
    }
    if let Some(r) = &result {
        if let Some(id) = &r.identity {
            if id.generation != generation || id.launch_id != launch_id {
                return Err(ParentLossLedgerError::RecoveryRequired(
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                ));
            }
        }
    }
    if let Some(a) = &acknowledgement {
        if a.identity.generation != generation || a.identity.launch_id != launch_id {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
            ));
        }
    }
    if let Some(s) = &spawn_identity {
        if s.identity.generation != generation || s.identity.launch_id != launch_id {
            return Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
            ));
        }
    }

    let service = intent.as_ref().and_then(|i| i.service);
    let mut parsed_result_state = None;
    let (identity, finding) = match result {
        Some(result) => match &result.state {
            AdmissionResultState::Admitted => {
                parsed_result_state = Some("admitted".to_string());
                match result
                    .identity
                    .or_else(|| acknowledgement.map(|ack| ack.identity))
                    .or_else(|| spawn_identity.map(|s| s.identity))
                {
                    Some(id) => (Some(id), None),
                    None => (None, Some(AdmissionFinding::Unidentified)),
                }
            }
            AdmissionResultState::RejectedUnreaped { .. } => {
                parsed_result_state = Some("rejected_unreaped".to_string());
                match result
                    .identity
                    .or_else(|| acknowledgement.map(|ack| ack.identity))
                    .or_else(|| spawn_identity.map(|s| s.identity))
                {
                    Some(id) => (Some(id), None),
                    None => (None, Some(AdmissionFinding::Unidentified)),
                }
            }
            AdmissionResultState::SpawnFailed { .. } => {
                parsed_result_state = Some("spawn_failed".to_string());
                (None, Some(AdmissionFinding::SpawnFailed))
            }
            AdmissionResultState::RejectedAndReaped { .. } => {
                parsed_result_state = Some("rejected_and_reaped".to_string());
                (None, Some(AdmissionFinding::RejectedAndReaped))
            }
        },
        None => match acknowledgement {
            Some(ack) => (Some(ack.identity), None),
            None => match spawn_identity {
                Some(sp) => (Some(sp.identity), None),
                None => match sources.environment_identity.clone() {
                    Some(id) => (Some(id), None),
                    None => (None, Some(AdmissionFinding::NeverAcknowledged)),
                },
            },
        },
    };

    let parent_launch_id = identity
        .as_ref()
        .and_then(|id| id.parent_launch_id.clone())
        .or_else(|| intent.as_ref().and_then(|i| i.parent_launch_id.clone()));

    let (instance, uid, finding) = match identity {
        Some(id) => (
            Some(id.instance),
            Some(id.uid),
            finding.unwrap_or(AdmissionFinding::Exited),
        ),
        None => (
            None,
            None,
            finding.unwrap_or(AdmissionFinding::Unidentified),
        ),
    };

    Ok(DerivedAdmissionDrops {
        sources: sources.clone(),
        service,
        instance,
        uid,
        parent_launch_id,
        finding,
        parsed_result_state,
        digests,
    })
}

#[allow(dead_code)]
enum SingleExitVerdict {
    Exited,
    Settled(AdmissionFinding),
    Unverifiable,
    SameLive,
}

fn wait_single_for_exit(
    authority: &ClosingAuthority<'_>,
    instance: ProcessInstance,
    uid: Option<u32>,
    until: Instant,
) -> SingleExitVerdict {
    loop {
        match authority.source.observe(&instance) {
            InstanceVerdict::NotSameOrExited => return SingleExitVerdict::Exited,
            InstanceVerdict::SameLive { .. } => {
                if Instant::now() >= until {
                    return SingleExitVerdict::SameLive;
                }
            }
            InstanceVerdict::Unverifiable => {
                match settle_unverifiable(authority, instance.pid, uid) {
                    Some(finding) => return SingleExitVerdict::Settled(finding),
                    None => {
                        if Instant::now() >= until {
                            return SingleExitVerdict::Unverifiable;
                        }
                    }
                }
            }
        }
        thread::sleep(OBSERVATION_POLL.min(until.saturating_duration_since(Instant::now())));
    }
}

fn flush_batch_and_ids(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    seq: u64,
    batch_buffer: &mut Vec<CompletedBatchRow>,
    completed_ids: &mut HashSet<String>,
) -> Result<(), ParentLossLedgerError> {
    if batch_buffer.is_empty() {
        return Ok(());
    }
    let batch_file = RecoveryBatchFile {
        schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
        kind: RecoveryArtifactKind::Batch,
        generation,
        sealed_phase: ParentLossPhase::Sealed,
        seq,
        entries: batch_buffer.clone(),
    };
    let batch_path = ledger.closure_recovery_batch_path(generation, seq);
    write_json(&batch_path, &batch_file, json_options())?;

    let ids: Vec<String> = batch_buffer.iter().map(|e| e.launch_id.clone()).collect();
    let ids_file = RecoveryIdsFile {
        schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
        kind: RecoveryArtifactKind::Ids,
        generation,
        sealed_phase: ParentLossPhase::Sealed,
        seq,
        ids: ids.clone(),
    };
    let ids_path = ledger.closure_recovery_ids_path(generation, seq);
    write_json(&ids_path, &ids_file, json_options())?;

    let batch_bytes = fs::read(&batch_path)?;
    let batch_bytes_digest = digest_bytes(&batch_bytes);
    let receipt = RecoveryBatchReceipt {
        schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
        kind: RecoveryArtifactKind::Receipt,
        generation,
        sealed_phase: ParentLossPhase::Sealed,
        seq,
        batch_bytes_digest,
        ids: ids.clone(),
    };
    let receipt_path = ledger.closure_recovery_receipt_path(generation, seq);
    if let Some(parent) = receipt_path.parent() {
        fs::create_dir_all(parent)?;
    }
    write_json(&receipt_path, &receipt, json_options())?;

    for row in batch_buffer.drain(..) {
        let live_path = ledger.closure_recovery_live_path(generation, &row.launch_id);
        if live_path.is_file() {
            let _ = fs::remove_file(&live_path);
        }
        let prov_path = ledger.closure_recovery_provisional_path(generation, &row.launch_id);
        if prov_path.is_file() {
            let _ = fs::remove_file(&prov_path);
        }
        completed_ids.insert(row.launch_id.clone());
    }

    #[cfg(feature = "test-hooks")]
    if std::env::var("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM").as_deref() == Ok("after-batch") {
        loop {
            thread::sleep(Duration::from_millis(100));
        }
    }

    Ok(())
}

fn census_refusal(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    reason: &str,
) -> Result<(), ParentLossLedgerError> {
    write_json(
        ledger.census_refusal_path(generation),
        &CensusRefusalFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            generation,
            sealed_phase: ParentLossPhase::Sealed,
            reason: reason.to_owned(),
        },
        json_options(),
    )?;
    Ok(())
}

fn read_retirement_census(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> Result<CensusObligationFile, ParentLossLedgerError> {
    if has_census_refusal_or_wedged(ledger, generation) {
        return Err(unverifiable_admission());
    }
    let path = ledger.census_obligation_path(generation);
    if !path.exists() {
        if ledger
            .generation_path(generation)
            .join("census-started.json")
            .exists()
        {
            census_refusal(
                ledger,
                generation,
                "authoritative census lost after retirement began",
            )?;
            return Err(unverifiable_admission());
        }
        return Ok(CensusObligationFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            generation,
            sealed_phase: ParentLossPhase::Sealed,
            obligations: HashMap::new(),
            binding_digest: compute_census_obligation_digest(&HashMap::new())?,
        });
    }
    let valid = match read_json_durable::<CensusObligationFile>(
        ArtifactId::ParentLossCensusObligation,
        &path,
    ) {
        Ok(DurableRead::Present(file))
            if file.schema == PARENT_LOSS_RECOVERY_SCHEMA_V1
                && file.generation == generation
                && file.sealed_phase == ParentLossPhase::Sealed
                && compute_census_obligation_digest(&file.obligations)
                    .ok()
                    .as_ref()
                    == Some(&file.binding_digest)
                && file.obligations.iter().all(|(id, live)| {
                    validate_launch_id(id).is_ok()
                        && live.launch_id == *id
                        && live.schema == PARENT_LOSS_RECOVERY_SCHEMA_V1
                        && live.generation == generation
                        && live.sealed_phase == ParentLossPhase::Sealed
                        && live.kind == RecoveryArtifactKind::Live
                }) =>
        {
            Some(file)
        }
        _ => None,
    };
    match valid {
        Some(file) => Ok(file),
        None => {
            census_refusal(
                ledger,
                generation,
                "retirement census missing, damaged or contradictory",
            )?;
            Err(unverifiable_admission())
        }
    }
}

fn persist_live(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    live: &LiveRecoveryFile,
) -> Result<(), ParentLossLedgerError> {
    let mut live = live.clone();
    live.binding_digest = compute_live_binding_digest(live.root, live.uid, &live.descendants)?;
    let mut file = read_retirement_census(ledger, generation)?;
    file.obligations
        .insert(live.launch_id.clone(), live.clone());
    file.binding_digest = compute_census_obligation_digest(&file.obligations)?;
    // The authoritative census is durable first, including deadlines and source binding.
    write_json(
        ledger.census_obligation_path(generation),
        &file,
        json_options(),
    )?;
    let marker = ledger
        .generation_path(generation)
        .join("census-started.json");
    if !marker.exists() {
        write_json(
            marker,
            &serde_json::json!({"schema": 1, "generation": generation}),
            json_options(),
        )?;
    }
    write_json(
        ledger.closure_recovery_live_path(generation, &live.launch_id),
        &live,
        json_options(),
    )?;
    Ok(())
}

fn discharge_census(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<(), ParentLossLedgerError> {
    let mut file = read_retirement_census(ledger, generation)?;
    file.obligations.remove(launch_id);
    file.binding_digest = compute_census_obligation_digest(&file.obligations)?;
    write_json(
        ledger.census_obligation_path(generation),
        &file,
        json_options(),
    )?;
    Ok(())
}

fn begin_retained_descendants(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    authority: &ClosingAuthority<'_>,
    live: &mut LiveRecoveryFile,
) -> Result<Option<ClosureOutcome>, ParentLossLedgerError> {
    for child in &live.descendants {
        if matches!(
            authority.source.observe(&child.instance),
            InstanceVerdict::SameLive { .. }
        ) && authority.retirer.owner(child.instance.pid) != ProcessOwner::Uid(child.uid)
        {
            return Ok(Some(ClosureOutcome::Refused(
                BootstrapRecoveryReason::AbandonedAdmissionLive,
            )));
        }
    }
    for index in 0..live.descendants.len() {
        let child = live.descendants[index].instance;
        if !live.descendants[index].term_requested
            && matches!(
                authority.source.observe(&child),
                InstanceVerdict::SameLive { .. }
            )
        {
            live.descendants[index].term_requested = true;
            live.descendants[index].escalate_at_unix_ms = unix_millis() + LIVE_ESCALATION_GRACE_MS;
            persist_live(ledger, generation, live)?;
            if authority.retirer.owner(child.pid) != ProcessOwner::Uid(live.uid) {
                return Ok(Some(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::AbandonedAdmissionLive,
                )));
            }
            let _ = authority.retirer.signal(child, SignalKind::Terminate);
        }
    }
    live.binding_digest = compute_live_binding_digest(live.root, live.uid, &live.descendants)?;
    Ok(None)
}

fn discharge_retained_descendants(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    authority: &ClosingAuthority<'_>,
    live_file: &mut LiveRecoveryFile,
    _live_path: &Path,
) -> Result<Option<ClosureOutcome>, ParentLossLedgerError> {
    let mut any_descendant_live = false;
    let mut need_persist = false;

    for i in 0..live_file.descendants.len() {
        let inst = live_file.descendants[i].instance;
        let uid = live_file.descendants[i].uid;
        match authority.source.observe(&inst) {
            InstanceVerdict::SameLive { .. } => {
                if uid != live_file.uid
                    || authority.retirer.owner(inst.pid) != ProcessOwner::Uid(uid)
                {
                    return Ok(Some(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::AbandonedAdmissionLive,
                    )));
                }
                let now_ms = unix_millis();
                if live_file.descendants[i].escalate_at_unix_ms == 0
                    || live_file.descendants[i].escalate_at_unix_ms
                        > now_ms + LIVE_ESCALATION_GRACE_MS
                {
                    live_file.descendants[i].escalate_at_unix_ms =
                        now_ms + LIVE_ESCALATION_GRACE_MS;
                    need_persist = true;
                }

                if !live_file.descendants[i].term_requested {
                    live_file.descendants[i].term_requested = true;
                    live_file.descendants[i].escalate_at_unix_ms =
                        unix_millis() + LIVE_ESCALATION_GRACE_MS;
                    persist_live(ledger, generation, live_file)?;
                    need_persist = false;
                }
                if authority.retirer.owner(inst.pid) != ProcessOwner::Uid(uid) {
                    return Ok(Some(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::AbandonedAdmissionLive,
                    )));
                }
                let _ = authority.retirer.signal(inst, SignalKind::Terminate);

                let now_ms = unix_millis();
                let esc_ms = live_file.descendants[i].escalate_at_unix_ms;
                if now_ms < esc_ms {
                    let wait_ms = esc_ms - now_ms;
                    let wait_until =
                        (Instant::now() + Duration::from_millis(wait_ms)).min(authority.deadline);
                    let exit_v = wait_single_for_exit(authority, inst, Some(uid), wait_until);
                    if matches!(exit_v, SingleExitVerdict::SameLive)
                        && Instant::now() >= authority.deadline
                    {
                        return Ok(Some(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::CoordinatorNotLive,
                        )));
                    }
                }

                let now_ms = unix_millis();
                if matches!(
                    authority.source.observe(&inst),
                    InstanceVerdict::SameLive { .. }
                ) && now_ms >= live_file.descendants[i].escalate_at_unix_ms
                {
                    live_file.descendants[i].escalated = true;
                    let target_reap = now_ms + LIVE_REAP_GRACE_MS;
                    match live_file.descendants[i].reap_deadline_unix_ms {
                        None => {
                            live_file.descendants[i].reap_deadline_unix_ms = Some(target_reap);
                        }
                        Some(stored) if stored > target_reap => {
                            live_file.descendants[i].reap_deadline_unix_ms = Some(target_reap);
                        }
                        _ => {}
                    }
                    persist_live(ledger, generation, live_file)?;
                    need_persist = false;
                    if authority.retirer.owner(inst.pid) != ProcessOwner::Uid(uid) {
                        return Ok(Some(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionLive,
                        )));
                    }
                    let _ = authority.retirer.signal(inst, SignalKind::Kill);

                    if let Some(reap_ms) = live_file.descendants[i].reap_deadline_unix_ms {
                        let cur_ms = unix_millis();
                        if cur_ms < reap_ms {
                            let wait_ms = reap_ms - cur_ms;
                            let wait_until = (Instant::now() + Duration::from_millis(wait_ms))
                                .min(authority.deadline);
                            let _ = wait_single_for_exit(authority, inst, Some(uid), wait_until);
                        }
                        if unix_millis() >= reap_ms
                            && matches!(
                                authority.source.observe(&inst),
                                InstanceVerdict::SameLive { .. }
                            )
                        {
                            return Ok(Some(ClosureOutcome::Refused(
                                BootstrapRecoveryReason::AbandonedAdmissionLive,
                            )));
                        }
                    }
                }

                if matches!(
                    authority.source.observe(&inst),
                    InstanceVerdict::SameLive { .. }
                ) {
                    any_descendant_live = true;
                }
            }
            InstanceVerdict::Unverifiable => {
                match settle_unverifiable(authority, inst.pid, Some(uid)) {
                    Some(AdmissionFinding::Exited)
                    | Some(AdmissionFinding::PidReusedByAnotherUser) => {}
                    _ => {
                        return Ok(Some(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                        )));
                    }
                }
            }
            InstanceVerdict::NotSameOrExited => {}
        }
        // A later wait may lose observability; only positive settlement discharges it.
        match authority.source.observe(&inst) {
            InstanceVerdict::NotSameOrExited => {}
            InstanceVerdict::SameLive { .. } => any_descendant_live = true,
            InstanceVerdict::Unverifiable => {
                if !matches!(
                    settle_unverifiable(authority, inst.pid, Some(uid)),
                    Some(AdmissionFinding::Exited | AdmissionFinding::PidReusedByAnotherUser)
                ) {
                    return Ok(Some(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                    )));
                }
            }
        }
    }

    if need_persist {
        persist_live(ledger, generation, live_file)?;
    }

    if any_descendant_live {
        return Ok(Some(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionLive,
        )));
    }

    Ok(None)
}

fn run_live_state_machine(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    authority: &ClosingAuthority<'_>,
    mut live_file: LiveRecoveryFile,
    batch_buffer: &mut Vec<CompletedBatchRow>,
    next_seq: &mut u64,
    completed_ids: &mut HashSet<String>,
) -> Result<Option<ClosureOutcome>, ParentLossLedgerError> {
    let live_path = ledger.closure_recovery_live_path(generation, &live_file.launch_id);

    if has_census_refusal_or_wedged(ledger, generation) {
        return Ok(Some(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
        )));
    }

    let census = read_retirement_census(ledger, generation)?;
    if let Some(retained) = census.obligations.get(&live_file.launch_id) {
        live_file = retained.clone();
    } else if live_file.term_requested || live_file.escalated || !live_file.descendants.is_empty() {
        census_refusal(
            ledger,
            generation,
            "missing obligation after retirement began",
        )?;
        return Err(unverifiable_admission());
    }

    let derived = match &live_file.sources {
        Some(sources)
            if sources.environment_identity.is_some()
                && sources.environment_journal.as_deref() != ledger.canonical_root().to_str() =>
        {
            census_refusal(
                ledger,
                generation,
                "environment observation belongs to another journal",
            )?;
            return Err(unverifiable_admission());
        }
        Some(sources) => derive_admission_source_facts(generation, &live_file.launch_id, sources)?,
        None => read_derived_admission_drops(ledger, generation, &live_file.launch_id)?,
    };
    if derived.instance != Some(live_file.root)
        || derived.uid != Some(live_file.uid)
        || derived.service != live_file.service
        || derived.parent_launch_id != live_file.parent_launch_id
    {
        census_refusal(
            ledger,
            generation,
            "retirement root is not authorized by its source evidence",
        )?;
        return Err(unverifiable_admission());
    }
    live_file.sources = Some(derived.sources.clone());

    #[cfg(any(test, feature = "test-hooks"))]
    if crate::process::retirement_skip_test_fault()
        && matches!(
            observe_settled(authority.source, &live_file.root, authority.deadline),
            InstanceVerdict::SameLive { .. }
        )
    {
        return Ok(Some(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionLive,
        )));
    }

    let mut verdict = authority.source.observe(&live_file.root);

    if matches!(verdict, InstanceVerdict::SameLive { .. }) {
        {
            #[cfg(any(target_os = "linux", target_os = "macos"))]
            {
                let snapshot =
                    match exact_descendant_tree(live_file.root, live_file.uid, authority.source) {
                        Ok(s) => s,
                        Err(_) => {
                            return Ok(Some(ClosureOutcome::Refused(
                                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                            )));
                        }
                    };
                let descendants: Vec<LiveDescendantIdentity> = snapshot
                    .descendants
                    .into_iter()
                    .filter_map(|d| {
                        let birth = snapshot.descendant_births.get(&d.pid).copied()?;
                        let pid = u32::try_from(d.pid).ok()?;
                        let ppid = u32::try_from(d.ppid).unwrap_or(0);
                        let pgid = d.pgid.and_then(|g| u32::try_from(g).ok()).unwrap_or(0);
                        Some(LiveDescendantIdentity {
                            instance: ProcessInstance { pid, birth },
                            uid: d.uid,
                            ppid,
                            pgid,
                            escalate_at_unix_ms: 0,
                            reap_deadline_unix_ms: None,
                            term_requested: false,
                            escalated: false,
                        })
                    })
                    .collect();
                for mut descendant in descendants {
                    if !live_file
                        .descendants
                        .iter()
                        .any(|old| old.instance == descendant.instance)
                    {
                        descendant.escalate_at_unix_ms = unix_millis() + LIVE_ESCALATION_GRACE_MS;
                        live_file.descendants.push(descendant);
                    }
                }
            }

            live_file.binding_digest =
                compute_live_binding_digest(live_file.root, live_file.uid, &live_file.descendants)?;

            let now_ms = unix_millis();
            if live_file.escalate_at_unix_ms == 0
                || live_file.escalate_at_unix_ms > now_ms + LIVE_ESCALATION_GRACE_MS
            {
                live_file.escalate_at_unix_ms = now_ms + LIVE_ESCALATION_GRACE_MS;
            }
            persist_live(ledger, generation, &live_file)?;
        }

        if let Some(outcome) =
            begin_retained_descendants(ledger, generation, authority, &mut live_file)?
        {
            return Ok(Some(outcome));
        }
        if !live_file.term_requested {
            live_file.term_requested = true;
            live_file.escalate_at_unix_ms = unix_millis() + LIVE_ESCALATION_GRACE_MS;
            persist_live(ledger, generation, &live_file)?;
            if authority.retirer.owner(live_file.root.pid) == ProcessOwner::Uid(live_file.uid) {
                let _ = authority
                    .retirer
                    .signal(live_file.root, SignalKind::Terminate);
            }
        } else {
            if authority.retirer.owner(live_file.root.pid) == ProcessOwner::Uid(live_file.uid) {
                let _ = authority
                    .retirer
                    .signal(live_file.root, SignalKind::Terminate);
            }
        }

        #[cfg(feature = "test-hooks")]
        if std::env::var("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM").as_deref()
            == Ok("after-retirement-term")
        {
            loop {
                thread::sleep(Duration::from_millis(100));
            }
        }
        verdict = authority.source.observe(&live_file.root);
        while matches!(verdict, InstanceVerdict::SameLive { .. }) {
            let now_ms = unix_millis();
            if now_ms >= live_file.escalate_at_unix_ms {
                break;
            }
            let wait_ms = live_file.escalate_at_unix_ms - now_ms;
            let escalate_instant = Instant::now() + Duration::from_millis(wait_ms);
            let wait_until = escalate_instant.min(authority.deadline);
            let exit_v =
                wait_single_for_exit(authority, live_file.root, Some(live_file.uid), wait_until);
            if matches!(exit_v, SingleExitVerdict::SameLive) {
                if Instant::now() >= authority.deadline {
                    return Ok(Some(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::CoordinatorNotLive,
                    )));
                }
            }
            verdict = authority.source.observe(&live_file.root);
        }

        let now_ms = unix_millis();
        if matches!(verdict, InstanceVerdict::SameLive { .. })
            && now_ms >= live_file.escalate_at_unix_ms
        {
            let mut need_write = false;
            match live_file.reap_deadline_unix_ms {
                None => {
                    live_file.reap_deadline_unix_ms = Some(now_ms + LIVE_REAP_GRACE_MS);
                    need_write = true;
                }
                Some(stored) if stored > now_ms + LIVE_REAP_GRACE_MS => {
                    live_file.reap_deadline_unix_ms = Some(now_ms + LIVE_REAP_GRACE_MS);
                    need_write = true;
                }
                _ => {}
            }
            if !live_file.escalated {
                live_file.escalated = true;
                need_write = true;
            }
            if need_write {
                persist_live(ledger, generation, &live_file)?;
            }
            if authority.retirer.owner(live_file.root.pid) == ProcessOwner::Uid(live_file.uid) {
                let _ = authority.retirer.signal(live_file.root, SignalKind::Kill);
            }

            let reap_ms = live_file.reap_deadline_unix_ms.unwrap();
            let cur_ms = unix_millis();
            if cur_ms < reap_ms {
                let wait_ms = reap_ms - cur_ms;
                let reap_instant = Instant::now() + Duration::from_millis(wait_ms);
                let wait_until = reap_instant.min(authority.deadline);
                let _ = wait_single_for_exit(
                    authority,
                    live_file.root,
                    Some(live_file.uid),
                    wait_until,
                );
            }
            if unix_millis() >= reap_ms
                && matches!(
                    authority.source.observe(&live_file.root),
                    InstanceVerdict::SameLive { .. }
                )
            {
                return Ok(Some(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::AbandonedAdmissionLive,
                )));
            }
            verdict = authority.source.observe(&live_file.root);
        }
    }

    match verdict {
        InstanceVerdict::NotSameOrExited => {
            let expected_digest =
                compute_live_binding_digest(live_file.root, live_file.uid, &live_file.descendants)?;
            if live_file.binding_digest != expected_digest {
                return Ok(Some(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                )));
            }

            if let Some(outcome) = discharge_retained_descendants(
                ledger,
                generation,
                authority,
                &mut live_file,
                &live_path,
            )? {
                return Ok(Some(outcome));
            }

            let finding = AdmissionFinding::Retired {
                escalated: live_file.escalated,
            };
            let binding_digest = compute_batch_row_binding_digest(
                &live_file.launch_id,
                derived.service,
                Some(live_file.root),
                Some(live_file.uid),
                derived.parent_launch_id.as_deref(),
                finding,
                derived.parsed_result_state.as_deref(),
                derived.digests.intent.as_deref(),
                derived.digests.result.as_deref(),
                derived.digests.ack.as_deref(),
                derived.digests.spawn_identity.as_deref(),
            )?;

            let row = CompletedBatchRow {
                sources: Some(derived.sources.clone()),
                launch_id: live_file.launch_id.clone(),
                service: derived.service,
                instance: Some(live_file.root),
                uid: Some(live_file.uid),
                parent_launch_id: derived.parent_launch_id,
                finding,
                parsed_result_state: derived.parsed_result_state,
                intent_digest: derived.digests.intent,
                result_digest: derived.digests.result,
                ack_digest: derived.digests.ack,
                spawn_identity_digest: derived.digests.spawn_identity,
                binding_digest,
            };
            batch_buffer.push(row);
            flush_batch_and_ids(ledger, generation, *next_seq, batch_buffer, completed_ids)?;
            *next_seq += 1;
            discharge_census(ledger, generation, &live_file.launch_id)?;
            let _ = fs::remove_file(&live_path);
            Ok(None)
        }
        InstanceVerdict::Unverifiable => {
            match settle_unverifiable(authority, live_file.root.pid, Some(live_file.uid)) {
                Some(finding) => {
                    let expected_digest = compute_live_binding_digest(
                        live_file.root,
                        live_file.uid,
                        &live_file.descendants,
                    )?;
                    if live_file.binding_digest != expected_digest {
                        return Ok(Some(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                        )));
                    }

                    if let Some(outcome) = discharge_retained_descendants(
                        ledger,
                        generation,
                        authority,
                        &mut live_file,
                        &live_path,
                    )? {
                        return Ok(Some(outcome));
                    }

                    let binding_digest = compute_batch_row_binding_digest(
                        &live_file.launch_id,
                        derived.service,
                        Some(live_file.root),
                        Some(live_file.uid),
                        derived.parent_launch_id.as_deref(),
                        finding,
                        derived.parsed_result_state.as_deref(),
                        derived.digests.intent.as_deref(),
                        derived.digests.result.as_deref(),
                        derived.digests.ack.as_deref(),
                        derived.digests.spawn_identity.as_deref(),
                    )?;
                    let row = CompletedBatchRow {
                        sources: Some(derived.sources.clone()),
                        launch_id: live_file.launch_id.clone(),
                        service: derived.service,
                        instance: Some(live_file.root),
                        uid: Some(live_file.uid),
                        parent_launch_id: derived.parent_launch_id,
                        finding,
                        parsed_result_state: derived.parsed_result_state,
                        intent_digest: derived.digests.intent,
                        result_digest: derived.digests.result,
                        ack_digest: derived.digests.ack,
                        spawn_identity_digest: derived.digests.spawn_identity,
                        binding_digest,
                    };
                    batch_buffer.push(row);
                    flush_batch_and_ids(
                        ledger,
                        generation,
                        *next_seq,
                        batch_buffer,
                        completed_ids,
                    )?;
                    *next_seq += 1;
                    discharge_census(ledger, generation, &live_file.launch_id)?;
                    let _ = fs::remove_file(&live_path);
                    Ok(None)
                }
                None => Ok(Some(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                ))),
            }
        }
        InstanceVerdict::SameLive { .. } => Ok(Some(ClosureOutcome::Refused(
            BootstrapRecoveryReason::CoordinatorNotLive,
        ))),
    }
}

pub(crate) fn completed_recovery(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    authority: &ClosingAuthority<'_>,
) -> Result<bool, ParentLossLedgerError> {
    publication::completed(ledger, generation, authority)
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
        mut record,
        set_aside: mut set_aside_files,
        notes,
    } = abandoned;

    // 1. SameLive supervisor and coordinator vetoes first.
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

    if publication::completed(ledger, generation, authority)? {
        return Ok(ClosureOutcome::Closed);
    }
    if record.as_ref().is_some_and(|r| r.closure.is_some())
        && let Some(aside) = publication::supersede_overtaken_publication(ledger, generation)?
    {
        if let Some(name) = aside.file_name() {
            set_aside_files.push(name.to_string_lossy().into_owned());
        }
        record = None;
    }
    if record.as_ref().is_some_and(|r| r.closure.is_some()) {
        if ledger
            .closure_recovery_phase_dir(generation)
            .join("publication.json")
            .exists()
        {
            return Err(unverifiable_admission());
        }
        // Older releases published closure without a reusable exact-byte proof.
        // Re-audit those admissions; their self-reported digest alone is insufficient.
        record.as_mut().expect("legacy record present").closure = None;
    }

    if has_census_refusal_or_wedged(ledger, generation) {
        return Ok(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
        ));
    }

    // 2. Mark sealed for closure before scan.
    ledger.mark_sealed_for_closure(generation)?;

    // 3. Legacy closure-recovery.json set-aside.
    let legacy_path = ledger.closure_recovery_legacy_path(generation);
    if legacy_path.is_file() {
        if let Ok(aside) = set_aside(&legacy_path) {
            if let Some(file_name) = aside.file_name() {
                let name = file_name.to_string_lossy().into_owned();
                if !set_aside_files.contains(&name) {
                    set_aside_files.push(name);
                }
            }
        }
    }

    // 4. Create directory structure.
    fs::create_dir_all(ledger.closure_recovery_batches_dir(generation))?;
    fs::create_dir_all(ledger.closure_recovery_ids_dir(generation))?;
    fs::create_dir_all(ledger.closure_recovery_live_dir(generation))?;
    fs::create_dir_all(ledger.closure_recovery_provisional_dir(generation))?;
    fs::create_dir_all(ledger.closure_recovery_phase_dir(generation))?;

    // 5. Hold admissions lock.
    let _lock = hold_lock(
        ledger.admission_lock_path(generation),
        LockOptions {
            timeout: ADMISSION_LOCK_TIMEOUT,
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )?;

    // Retained live obligations have priority over every historical payload.
    let mut max_seq = fs::read_dir(ledger.closure_recovery_batches_dir(generation))?
        .filter_map(Result::ok)
        .filter_map(|e| e.path().file_stem()?.to_str()?.parse::<u64>().ok())
        .max()
        .unwrap_or(0);
    let mut next_seq = max_seq + 1;
    let mut current_batch_buffer: Vec<CompletedBatchRow> = Vec::new();
    let mut completed_ids: HashSet<String> = HashSet::new();
    // 9. Process live files before read_dir.
    let live_dir = ledger.closure_recovery_live_dir(generation);
    let mut live_files = Vec::new();
    let census = read_retirement_census(ledger, generation)?;
    if let Ok(entries) = fs::read_dir(&live_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(id) = path.file_stem().and_then(|s| s.to_str())
                && let Some(retained) = census.obligations.get(id)
            {
                live_files.push(retained.clone());
                continue;
            }
            match read_json_durable_validated::<LiveRecoveryFile>(
                ArtifactId::ParentLossClosureRecovery,
                &path,
                |l| {
                    if l.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                        || l.generation != generation
                        || l.sealed_phase != ParentLossPhase::Sealed
                        || l.kind != RecoveryArtifactKind::Live
                    {
                        return Err("invalid live envelope".to_string());
                    }
                    Ok(())
                },
            ) {
                Ok(DurableRead::Present(live_file)) => {
                    live_files.push(live_file);
                }
                Ok(DurableRead::SetAside(aside)) => {
                    if let Some(name) = aside.file_name() {
                        set_aside_files.push(name.to_string_lossy().into_owned());
                    }
                    let launch_id = path
                        .file_stem()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_default();
                    if let Some(retained) = census.obligations.get(&launch_id) {
                        live_files.push(retained.clone());
                        continue;
                    }
                    let derived = read_derived_admission_drops(ledger, generation, &launch_id)?;
                    let is_gone = match derived.instance {
                        Some(inst) => matches!(
                            authority.source.observe(&inst),
                            InstanceVerdict::NotSameOrExited
                        ),
                        None => true,
                    };
                    if is_gone {
                        census_refusal(ledger, generation, "live cache damaged after root exit")?;
                        return Ok(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                        ));
                    }
                }
                _ => {}
            }
        }
    }

    let census = read_retirement_census(ledger, generation)?;
    for retained in census.obligations.into_values() {
        if let Some(index) = live_files
            .iter()
            .position(|l| l.launch_id == retained.launch_id)
        {
            live_files[index] = retained;
        } else {
            live_files.push(retained);
        }
    }

    for live_file in live_files {
        if let Some(outcome) = run_live_state_machine(
            ledger,
            generation,
            authority,
            live_file,
            &mut current_batch_buffer,
            &mut next_seq,
            &mut completed_ids,
        )? {
            return Ok(outcome);
        }
    }

    if next_seq == max_seq + 1
        && current_batch_buffer.is_empty()
        && let Some(outcome) =
            publication::resume_prepared(ledger, generation, authority, record.as_ref())?
    {
        return Ok(outcome.into_closure());
    }

    // 7. Load provisional files.
    let mut provisional_map: HashMap<String, ProvisionalFile> = HashMap::new();
    let prov_dir = ledger.closure_recovery_provisional_dir(generation);
    if let Ok(entries) = fs::read_dir(&prov_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match read_json_durable_validated::<ProvisionalFile>(
                ArtifactId::ParentLossClosureRecovery,
                &path,
                |p| {
                    if p.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                        || p.generation != generation
                        || p.sealed_phase != ParentLossPhase::Sealed
                        || p.kind != RecoveryArtifactKind::Provisional
                    {
                        return Err("invalid provisional envelope".to_string());
                    }
                    Ok(())
                },
            ) {
                Ok(DurableRead::Present(prov)) => {
                    provisional_map.insert(prov.launch_id.clone(), prov);
                }
                Ok(DurableRead::SetAside(aside)) => {
                    if let Some(name) = aside.file_name() {
                        set_aside_files.push(name.to_string_lossy().into_owned());
                    }
                }
                _ => {}
            }
        }
    }

    // A completed scan schedules publication; it never certifies its rows.
    // The publisher still authenticates actual IDs and committed output (or
    // audits original sources), checks provisionals and inventories the result.
    let scan_complete_path = ledger.closure_recovery_scan_complete_path(generation);
    if next_seq == max_seq + 1
        && current_batch_buffer.is_empty()
        && publication::current_provisional_sources(ledger, generation, &provisional_map)?.is_some()
    {
        match read_json_durable_validated::<ScanCompleteFile>(
            ArtifactId::ParentLossClosureRecovery,
            &scan_complete_path,
            |scan| {
                if scan.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                    || scan.generation != generation
                    || scan.sealed_phase != ParentLossPhase::Sealed
                    || scan.kind != RecoveryArtifactKind::ScanComplete
                {
                    return Err("invalid scan-complete envelope".to_owned());
                }
                Ok(())
            },
        )? {
            DurableRead::Present(scan)
                if scan.sorted_launch_ids == publication::inventory(ledger, generation)? =>
            {
                #[cfg(test)]
                SCAN_PUBLICATIONS.with(|count| count.set(count.get() + 1));
                let result = publication::publish(
                    ledger,
                    generation,
                    successor_generation,
                    authority,
                    supervisor,
                    coordinator,
                    record.clone(),
                    supervisor_observation,
                    coordinator_observation,
                    set_aside_files.clone(),
                    notes.clone(),
                    scan.sorted_launch_ids,
                    scan.ids_sequences,
                    &provisional_map,
                );
                match &result {
                    Ok(
                        publication::PublicationOutcome::Closed
                        | publication::PublicationOutcome::BudgetExpired
                        | publication::PublicationOutcome::InputsChanged,
                    ) => {
                        return result.map(publication::PublicationOutcome::into_closure);
                    }
                    #[cfg(test)]
                    Ok(publication::PublicationOutcome::Paused) => {
                        return result.map(publication::PublicationOutcome::into_closure);
                    }
                    Ok(publication::PublicationOutcome::RebuildRequired) | Err(_) => {}
                }
                // Invalid inputs invalidate the scheduling hint even if their
                // cleanup crosses the deadline. A genuine budget refusal keeps
                // the hint so the next start can resume publication directly.
                let aside = set_aside(&scan_complete_path)?;
                if let Some(name) = aside.file_name() {
                    set_aside_files.push(name.to_string_lossy().into_owned());
                }
                result?;
            }
            DurableRead::Present(_) => {
                let aside = set_aside(&scan_complete_path)?;
                if let Some(name) = aside.file_name() {
                    set_aside_files.push(name.to_string_lossy().into_owned());
                }
            }
            DurableRead::SetAside(aside) => {
                if let Some(name) = aside.file_name() {
                    set_aside_files.push(name.to_string_lossy().into_owned());
                }
            }
            _ => {}
        }
    }

    #[cfg(test)]
    {
        HISTORICAL_DISCOVERIES.with(|count| count.set(count.get() + 1));
        if DISCOVERY_STOP_FAULT.with(|fault| fault.get()) {
            return Ok(ClosureOutcome::Refused(
                BootstrapRecoveryReason::CoordinatorNotLive,
            ));
        }
    }

    completed_ids.clear();
    // 6. Discovery IDs are scheduling hints. Publication validates each payload
    // against its receipt and source facts before any hint can enter the audit.
    let mut raw_ids: HashMap<u64, (RecoveryIdsFile, std::path::PathBuf)> = HashMap::new();
    let mut batch_paths: HashMap<u64, std::path::PathBuf> = HashMap::new();

    let ids_dir = ledger.closure_recovery_ids_dir(generation);
    if let Ok(entries) = fs::read_dir(&ids_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            match read_json_durable_validated::<RecoveryIdsFile>(
                ArtifactId::ParentLossClosureRecovery,
                &path,
                |f| {
                    if f.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                        || f.generation != generation
                        || f.sealed_phase != ParentLossPhase::Sealed
                        || f.kind != RecoveryArtifactKind::Ids
                    {
                        return Err("invalid ids envelope".to_string());
                    }
                    Ok(())
                },
            ) {
                Ok(DurableRead::Present(ids_file)) => {
                    max_seq = max_seq.max(ids_file.seq);
                    raw_ids.insert(ids_file.seq, (ids_file, path));
                }
                Ok(DurableRead::SetAside(aside)) => {
                    if let Some(name) = aside.file_name() {
                        let name = name.to_string_lossy().into_owned();
                        if !set_aside_files.contains(&name) {
                            set_aside_files.push(name);
                        }
                    }
                }
                _ => {}
            }
        }
    }

    let batches_dir = ledger.closure_recovery_batches_dir(generation);
    if let Ok(entries) = fs::read_dir(&batches_dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("json") {
                continue;
            }
            if let Some(stem) = path.file_stem().and_then(|s| s.to_str()) {
                if let Ok(seq) = stem.parse::<u64>() {
                    max_seq = max_seq.max(seq);
                    batch_paths.insert(seq, path);
                }
            }
        }
    }

    let mut valid_ids_seqs: Vec<u64> = Vec::new();
    let mut sorted_seqs: Vec<u64> = raw_ids.keys().chain(batch_paths.keys()).copied().collect();
    sorted_seqs.sort_unstable();
    sorted_seqs.dedup();

    for seq in sorted_seqs {
        let ids_entry = raw_ids.remove(&seq);
        let batch_path = batch_paths.remove(&seq);

        match (ids_entry, batch_path) {
            (Some((ids_file, i_path)), Some(b_path)) => {
                let r_path = ledger.closure_recovery_receipt_path(generation, seq);
                let set_aside_all = |set_aside_files: &mut Vec<String>| {
                    if let Ok(aside) = set_aside(&i_path) {
                        if let Some(name) = aside.file_name() {
                            set_aside_files.push(name.to_string_lossy().into_owned());
                        }
                    }
                    if let Ok(aside) = set_aside(&b_path) {
                        if let Some(name) = aside.file_name() {
                            set_aside_files.push(name.to_string_lossy().into_owned());
                        }
                    }
                    if r_path.is_file() {
                        if let Ok(aside) = set_aside(&r_path) {
                            if let Some(name) = aside.file_name() {
                                set_aside_files.push(name.to_string_lossy().into_owned());
                            }
                        }
                    }
                };

                if r_path.is_file() {
                    let Ok(receipt_bytes) = fs::read(&r_path) else {
                        set_aside_all(&mut set_aside_files);
                        continue;
                    };
                    let receipt: RecoveryBatchReceipt = match serde_json::from_slice(&receipt_bytes)
                    {
                        Ok(r) => r,
                        Err(_) => {
                            set_aside_all(&mut set_aside_files);
                            continue;
                        }
                    };
                    if receipt.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                        || receipt.generation != generation
                        || receipt.sealed_phase != ParentLossPhase::Sealed
                        || receipt.kind != RecoveryArtifactKind::Receipt
                        || receipt.seq != seq
                        || receipt.ids != ids_file.ids
                    {
                        set_aside_all(&mut set_aside_files);
                        continue;
                    }
                } else {
                    set_aside_all(&mut set_aside_files);
                    continue;
                }

                let unique_ids: HashSet<_> = ids_file.ids.iter().collect();
                let all_valid = unique_ids.len() == ids_file.ids.len()
                    && ids_file.ids.iter().all(|id| validate_launch_id(id).is_ok());
                let has_dup = ids_file.ids.iter().any(|id| completed_ids.contains(id));
                if !all_valid || has_dup {
                    set_aside_all(&mut set_aside_files);
                    continue;
                }
                for id in ids_file.ids {
                    completed_ids.insert(id);
                }
                valid_ids_seqs.push(seq);
            }
            (Some((_, i_path)), None) => {
                if let Ok(aside) = set_aside(&i_path) {
                    if let Some(name) = aside.file_name() {
                        set_aside_files.push(name.to_string_lossy().into_owned());
                    }
                }
            }
            (None, Some(b_path)) => {
                match read_json_durable_validated::<RecoveryBatchFile>(
                    ArtifactId::ParentLossClosureRecovery,
                    &b_path,
                    |b| {
                        if b.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                            || b.generation != generation
                            || b.sealed_phase != ParentLossPhase::Sealed
                            || b.kind != RecoveryArtifactKind::Batch
                        {
                            return Err("invalid batch envelope".to_string());
                        }
                        Ok(())
                    },
                ) {
                    Ok(DurableRead::Present(batch)) => {
                        let all_auth = batch
                            .entries
                            .iter()
                            .all(|row| completed_row_is_authorized(ledger, generation, row));
                        let all_valid = batch
                            .entries
                            .iter()
                            .all(|e| validate_launch_id(&e.launch_id).is_ok());
                        let has_dup = batch
                            .entries
                            .iter()
                            .any(|e| completed_ids.contains(&e.launch_id));
                        if !all_auth || !all_valid || has_dup {
                            if let Ok(aside) = set_aside(&b_path) {
                                if let Some(name) = aside.file_name() {
                                    set_aside_files.push(name.to_string_lossy().into_owned());
                                }
                            }
                            continue;
                        }
                        let ids: Vec<String> =
                            batch.entries.iter().map(|e| e.launch_id.clone()).collect();
                        let ids_file = RecoveryIdsFile {
                            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
                            kind: RecoveryArtifactKind::Ids,
                            generation,
                            sealed_phase: ParentLossPhase::Sealed,
                            seq,
                            ids: ids.clone(),
                        };
                        let i_path = ledger.closure_recovery_ids_path(generation, seq);
                        if write_json(&i_path, &ids_file, json_options()).is_ok() {
                            for id in ids {
                                completed_ids.insert(id);
                            }
                            valid_ids_seqs.push(seq);
                        }
                    }
                    Ok(DurableRead::SetAside(aside)) => {
                        if let Some(name) = aside.file_name() {
                            set_aside_files.push(name.to_string_lossy().into_owned());
                        }
                    }
                    _ => {}
                }
            }
            (None, None) => {}
        }
    }

    next_seq = max_seq + 1;

    // 8. Check scan-complete.json.
    let mut skip_dir_scan = false;
    let scan_complete_path = ledger.closure_recovery_scan_complete_path(generation);
    if scan_complete_path.is_file() {
        match read_json_durable_validated::<ScanCompleteFile>(
            ArtifactId::ParentLossClosureRecovery,
            &scan_complete_path,
            |s| {
                if s.schema != PARENT_LOSS_RECOVERY_SCHEMA_V1
                    || s.generation != generation
                    || s.sealed_phase != ParentLossPhase::Sealed
                    || s.kind != RecoveryArtifactKind::ScanComplete
                {
                    return Err("invalid scan-complete envelope".to_string());
                }
                Ok(())
            },
        ) {
            Ok(DurableRead::Present(sc)) => {
                let mut expected_ids: Vec<String> = completed_ids
                    .iter()
                    .chain(provisional_map.keys())
                    .cloned()
                    .collect();
                expected_ids.sort();
                expected_ids.dedup();
                let expected_bytes = canonical_json(&expected_ids)?;
                let expected_digest = digest_bytes(&expected_bytes);

                let admissions_dir = admission_directory(ledger, generation);
                let dir_walk_res: Option<HashSet<String>> = (|| {
                    let mut names = HashSet::new();
                    match fs::read_dir(&admissions_dir) {
                        Ok(entries) => {
                            for entry in entries {
                                let entry = entry.ok()?;
                                let file_name = entry.file_name();
                                let s = file_name.to_str()?;
                                if validate_launch_id(s).is_err() {
                                    return None;
                                }
                                if entry.file_type().ok()?.is_dir() {
                                    names.insert(s.to_string());
                                }
                            }
                            Some(names)
                        }
                        Err(err) if err.kind() == io::ErrorKind::NotFound => Some(HashSet::new()),
                        Err(_) => None,
                    }
                })();

                let disk_ids_match = match dir_walk_res {
                    Some(disk_ids) => {
                        let sc_set: HashSet<String> =
                            sc.sorted_launch_ids.iter().cloned().collect();
                        disk_ids == sc_set
                    }
                    None => false,
                };

                if disk_ids_match
                    && sc.sorted_launch_ids == expected_ids
                    && sc.ids_sequences == valid_ids_seqs
                    && sc.ids_digest == expected_digest
                {
                    skip_dir_scan = true;
                } else if let Ok(aside) = set_aside(&scan_complete_path) {
                    if let Some(name) = aside.file_name() {
                        set_aside_files.push(name.to_string_lossy().into_owned());
                    }
                }
            }
            Ok(DurableRead::SetAside(aside)) => {
                if let Some(name) = aside.file_name() {
                    set_aside_files.push(name.to_string_lossy().into_owned());
                }
            }
            _ => {}
        }
    }

    // 10. Scan admissions directory if not skipped by valid scan-complete.
    let mut seen_admission_dirs: HashSet<String> = HashSet::new();
    if !skip_dir_scan {
        let admissions_dir = admission_directory(ledger, generation);
        let dir_entries = match fs::read_dir(&admissions_dir) {
            Ok(entries) => Some(entries),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(_) => {
                return Ok(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                ));
            }
        };

        if let Some(entries) = dir_entries {
            for entry in entries {
                let entry = match entry {
                    Ok(e) => e,
                    Err(_) => {
                        return Ok(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                        ));
                    }
                };
                let file_name = entry.file_name();
                let Some(launch_id_str) = file_name.to_str() else {
                    return Ok(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                    ));
                };
                if validate_launch_id(launch_id_str).is_err() {
                    return Ok(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                    ));
                }

                let is_dir = match entry.file_type() {
                    Ok(ft) => ft.is_dir(),
                    Err(_) => {
                        return Ok(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                        ));
                    }
                };
                if !is_dir {
                    continue;
                }
                seen_admission_dirs.insert(launch_id_str.to_owned());

                if completed_ids.contains(launch_id_str) {
                    continue;
                }
                let launch_id = launch_id_str.to_owned();

                if Instant::now() >= authority.deadline {
                    if !current_batch_buffer.is_empty() {
                        flush_batch_and_ids(
                            ledger,
                            generation,
                            next_seq,
                            &mut current_batch_buffer,
                            &mut completed_ids,
                        )?;
                    }
                    return Ok(ClosureOutcome::Refused(
                        BootstrapRecoveryReason::CoordinatorNotLive,
                    ));
                }

                #[cfg(feature = "test-hooks")]
                {
                    if std::env::var("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM").as_deref()
                        == Ok("during-inventory")
                    {
                        loop {
                            thread::sleep(Duration::from_millis(100));
                        }
                    }
                    if let Ok(val) = std::env::var("SOLSTONE_PARENT_LOSS_CLOSURE_SLOW_MS")
                        && let Ok(ms) = val.parse::<u64>()
                    {
                        thread::sleep(Duration::from_millis(ms));
                    }
                }

                let derived = read_derived_admission_drops(ledger, generation, &launch_id)?;

                match (derived.instance, derived.uid) {
                    (Some(instance), Some(uid)) => {
                        let verdict = authority.source.observe(&instance);
                        match verdict {
                            InstanceVerdict::NotSameOrExited => {
                                let finding = AdmissionFinding::Exited;
                                let binding_digest = compute_batch_row_binding_digest(
                                    &launch_id,
                                    derived.service,
                                    Some(instance),
                                    Some(uid),
                                    derived.parent_launch_id.as_deref(),
                                    finding,
                                    derived.parsed_result_state.as_deref(),
                                    derived.digests.intent.as_deref(),
                                    derived.digests.result.as_deref(),
                                    derived.digests.ack.as_deref(),
                                    derived.digests.spawn_identity.as_deref(),
                                )?;
                                let row = CompletedBatchRow {
                                    sources: Some(derived.sources.clone()),
                                    launch_id: launch_id.clone(),
                                    service: derived.service,
                                    instance: Some(instance),
                                    uid: Some(uid),
                                    parent_launch_id: derived.parent_launch_id,
                                    finding,
                                    parsed_result_state: derived.parsed_result_state,
                                    intent_digest: derived.digests.intent,
                                    result_digest: derived.digests.result,
                                    ack_digest: derived.digests.ack,
                                    spawn_identity_digest: derived.digests.spawn_identity,
                                    binding_digest,
                                };
                                current_batch_buffer.push(row);
                            }
                            InstanceVerdict::SameLive { .. } => {
                                let live_file = LiveRecoveryFile {
                                    sources: None,
                                    schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
                                    kind: RecoveryArtifactKind::Live,
                                    generation,
                                    sealed_phase: ParentLossPhase::Sealed,
                                    launch_id: launch_id.clone(),
                                    service: derived.service,
                                    root: instance,
                                    uid,
                                    parent_launch_id: derived.parent_launch_id,
                                    descendants: Vec::new(),
                                    escalate_at_unix_ms: 0,
                                    reap_deadline_unix_ms: None,
                                    term_requested: false,
                                    escalated: false,
                                    binding_digest: String::new(),
                                };
                                if let Some(outcome) = run_live_state_machine(
                                    ledger,
                                    generation,
                                    authority,
                                    live_file,
                                    &mut current_batch_buffer,
                                    &mut next_seq,
                                    &mut completed_ids,
                                )? {
                                    return Ok(outcome);
                                }
                            }
                            InstanceVerdict::Unverifiable => {
                                match settle_unverifiable(authority, instance.pid, Some(uid)) {
                                    Some(finding) => {
                                        let binding_digest = compute_batch_row_binding_digest(
                                            &launch_id,
                                            derived.service,
                                            Some(instance),
                                            Some(uid),
                                            derived.parent_launch_id.as_deref(),
                                            finding,
                                            derived.parsed_result_state.as_deref(),
                                            derived.digests.intent.as_deref(),
                                            derived.digests.result.as_deref(),
                                            derived.digests.ack.as_deref(),
                                            derived.digests.spawn_identity.as_deref(),
                                        )?;
                                        let row = CompletedBatchRow {
                                            sources: Some(derived.sources.clone()),
                                            launch_id: launch_id.clone(),
                                            service: derived.service,
                                            instance: Some(instance),
                                            uid: Some(uid),
                                            parent_launch_id: derived.parent_launch_id,
                                            finding,
                                            parsed_result_state: derived.parsed_result_state,
                                            intent_digest: derived.digests.intent,
                                            result_digest: derived.digests.result,
                                            ack_digest: derived.digests.ack,
                                            spawn_identity_digest: derived.digests.spawn_identity,
                                            binding_digest,
                                        };
                                        current_batch_buffer.push(row);
                                    }
                                    None => {
                                        return Ok(ClosureOutcome::Refused(
                                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                                        ));
                                    }
                                }
                            }
                        }
                    }
                    _ => match derived.finding {
                        AdmissionFinding::SpawnFailed | AdmissionFinding::RejectedAndReaped => {
                            let binding_digest = compute_batch_row_binding_digest(
                                &launch_id,
                                derived.service,
                                None,
                                None,
                                derived.parent_launch_id.as_deref(),
                                derived.finding,
                                derived.parsed_result_state.as_deref(),
                                derived.digests.intent.as_deref(),
                                derived.digests.result.as_deref(),
                                derived.digests.ack.as_deref(),
                                derived.digests.spawn_identity.as_deref(),
                            )?;
                            let row = CompletedBatchRow {
                                sources: Some(derived.sources.clone()),
                                launch_id: launch_id.clone(),
                                service: derived.service,
                                instance: None,
                                uid: None,
                                parent_launch_id: derived.parent_launch_id,
                                finding: derived.finding,
                                parsed_result_state: derived.parsed_result_state,
                                intent_digest: derived.digests.intent,
                                result_digest: derived.digests.result,
                                ack_digest: derived.digests.ack,
                                spawn_identity_digest: derived.digests.spawn_identity,
                                binding_digest,
                            };
                            current_batch_buffer.push(row);
                        }
                        finding => {
                            let prov_file = ProvisionalFile {
                                schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
                                kind: RecoveryArtifactKind::Provisional,
                                generation,
                                sealed_phase: ParentLossPhase::Sealed,
                                launch_id: launch_id.clone(),
                                service: derived.service,
                                finding,
                                intent_digest: derived.digests.intent,
                                result_digest: derived.digests.result,
                                ack_digest: derived.digests.ack,
                                spawn_identity_digest: derived.digests.spawn_identity,
                            };
                            let prov_path =
                                ledger.closure_recovery_provisional_path(generation, &launch_id);
                            write_json(&prov_path, &prov_file, json_options())?;
                            provisional_map.insert(launch_id.clone(), prov_file);

                            #[cfg(test)]
                            if PROVISIONAL_STOP_FAULT.with(|f| f.get()) {
                                return Ok(ClosureOutcome::Refused(
                                    BootstrapRecoveryReason::CoordinatorNotLive,
                                ));
                            }
                        }
                    },
                }

                if current_batch_buffer.len() >= RECOVERY_BATCH_LIMIT {
                    flush_batch_and_ids(
                        ledger,
                        generation,
                        next_seq,
                        &mut current_batch_buffer,
                        &mut completed_ids,
                    )?;
                    next_seq += 1;
                }
            }
        }

        if !current_batch_buffer.is_empty() {
            flush_batch_and_ids(
                ledger,
                generation,
                next_seq,
                &mut current_batch_buffer,
                &mut completed_ids,
            )?;
            next_seq += 1;
        }
    }

    // 11. Provisional drop recheck.
    let prov_keys: Vec<String> = provisional_map.keys().cloned().collect();
    for launch_id in prov_keys {
        if completed_ids.contains(&launch_id) {
            let prov_path = ledger.closure_recovery_provisional_path(generation, &launch_id);
            if prov_path.is_file() {
                let _ = fs::remove_file(&prov_path);
            }
            provisional_map.remove(&launch_id);
            continue;
        }
        let derived = read_derived_admission_drops(ledger, generation, &launch_id)?;
        if let (Some(instance), Some(uid)) = (derived.instance, derived.uid) {
            let verdict = authority.source.observe(&instance);
            match verdict {
                InstanceVerdict::NotSameOrExited => {
                    let finding = AdmissionFinding::Exited;
                    let binding_digest = compute_batch_row_binding_digest(
                        &launch_id,
                        derived.service,
                        Some(instance),
                        Some(uid),
                        derived.parent_launch_id.as_deref(),
                        finding,
                        derived.parsed_result_state.as_deref(),
                        derived.digests.intent.as_deref(),
                        derived.digests.result.as_deref(),
                        derived.digests.ack.as_deref(),
                        derived.digests.spawn_identity.as_deref(),
                    )?;
                    let row = CompletedBatchRow {
                        sources: Some(derived.sources.clone()),
                        launch_id: launch_id.clone(),
                        service: derived.service,
                        instance: Some(instance),
                        uid: Some(uid),
                        parent_launch_id: derived.parent_launch_id,
                        finding,
                        parsed_result_state: derived.parsed_result_state,
                        intent_digest: derived.digests.intent,
                        result_digest: derived.digests.result,
                        ack_digest: derived.digests.ack,
                        spawn_identity_digest: derived.digests.spawn_identity,
                        binding_digest,
                    };
                    current_batch_buffer.push(row);
                    provisional_map.remove(&launch_id);
                    let prov_path =
                        ledger.closure_recovery_provisional_path(generation, &launch_id);
                    if prov_path.is_file() {
                        let _ = fs::remove_file(&prov_path);
                    }
                }
                InstanceVerdict::SameLive { .. } => {
                    let live_file = LiveRecoveryFile {
                        sources: Some(derived.sources.clone()),
                        schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
                        kind: RecoveryArtifactKind::Live,
                        generation,
                        sealed_phase: ParentLossPhase::Sealed,
                        launch_id: launch_id.clone(),
                        service: derived.service,
                        root: instance,
                        uid,
                        parent_launch_id: derived.parent_launch_id,
                        descendants: Vec::new(),
                        escalate_at_unix_ms: 0,
                        reap_deadline_unix_ms: None,
                        term_requested: false,
                        escalated: false,
                        binding_digest: String::new(),
                    };
                    if let Some(outcome) = run_live_state_machine(
                        ledger,
                        generation,
                        authority,
                        live_file,
                        &mut current_batch_buffer,
                        &mut next_seq,
                        &mut completed_ids,
                    )? {
                        return Ok(outcome);
                    }
                    provisional_map.remove(&launch_id);
                }
                InstanceVerdict::Unverifiable => {
                    if let Some(finding) = settle_unverifiable(authority, instance.pid, Some(uid)) {
                        let binding_digest = compute_batch_row_binding_digest(
                            &launch_id,
                            derived.service,
                            Some(instance),
                            Some(uid),
                            derived.parent_launch_id.as_deref(),
                            finding,
                            derived.parsed_result_state.as_deref(),
                            derived.digests.intent.as_deref(),
                            derived.digests.result.as_deref(),
                            derived.digests.ack.as_deref(),
                            derived.digests.spawn_identity.as_deref(),
                        )?;
                        let row = CompletedBatchRow {
                            sources: Some(derived.sources.clone()),
                            launch_id: launch_id.clone(),
                            service: derived.service,
                            instance: Some(instance),
                            uid: Some(uid),
                            parent_launch_id: derived.parent_launch_id,
                            finding,
                            parsed_result_state: derived.parsed_result_state,
                            intent_digest: derived.digests.intent,
                            result_digest: derived.digests.result,
                            ack_digest: derived.digests.ack,
                            spawn_identity_digest: derived.digests.spawn_identity,
                            binding_digest,
                        };
                        current_batch_buffer.push(row);
                        provisional_map.remove(&launch_id);
                        let prov_path =
                            ledger.closure_recovery_provisional_path(generation, &launch_id);
                        if prov_path.is_file() {
                            let _ = fs::remove_file(&prov_path);
                        }
                    } else {
                        return Ok(ClosureOutcome::Refused(
                            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                        ));
                    }
                }
            }
        }
    }

    if !current_batch_buffer.is_empty() {
        flush_batch_and_ids(
            ledger,
            generation,
            next_seq,
            &mut current_batch_buffer,
            &mut completed_ids,
        )?;
    }

    // 12. Check live directory is empty.
    let live_dir = ledger.closure_recovery_live_dir(generation);
    if let Ok(entries) = fs::read_dir(&live_dir) {
        if entries
            .flatten()
            .any(|e| e.path().extension().and_then(|ext| ext.to_str()) == Some("json"))
        {
            return Ok(ClosureOutcome::Refused(
                BootstrapRecoveryReason::CoordinatorNotLive,
            ));
        }
    }

    if Instant::now() >= authority.deadline {
        return Ok(ClosureOutcome::Refused(
            BootstrapRecoveryReason::CoordinatorNotLive,
        ));
    }

    // 13. Write scan-complete.json.
    if !skip_dir_scan {
        let expected_set: HashSet<String> = completed_ids
            .iter()
            .chain(provisional_map.keys())
            .cloned()
            .collect();
        if seen_admission_dirs != expected_set {
            return Ok(ClosureOutcome::Refused(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
            ));
        }
    }

    let mut all_scanned_ids: Vec<String> = completed_ids
        .iter()
        .chain(provisional_map.keys())
        .cloned()
        .collect();
    all_scanned_ids.sort();
    all_scanned_ids.dedup();

    let mut ids_seqs = valid_ids_seqs.clone();
    for entry in fs::read_dir(ledger.closure_recovery_ids_dir(generation))? {
        let path = entry?.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json")
            && let Some(seq) = path
                .file_stem()
                .and_then(|s| s.to_str())
                .and_then(|s| s.parse::<u64>().ok())
        {
            ids_seqs.push(seq);
        }
    }
    ids_seqs.sort_unstable();
    ids_seqs.dedup();

    let ids_bytes = canonical_json(&all_scanned_ids)?;
    let ids_digest = digest_bytes(&ids_bytes);

    let sc_file = ScanCompleteFile {
        schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
        kind: RecoveryArtifactKind::ScanComplete,
        generation,
        sealed_phase: ParentLossPhase::Sealed,
        sorted_launch_ids: all_scanned_ids.clone(),
        ids_sequences: ids_seqs.clone(),
        ids_digest,
    };
    write_json(&scan_complete_path, &sc_file, json_options())?;

    publication::publish(
        ledger,
        generation,
        successor_generation,
        authority,
        supervisor,
        coordinator,
        record,
        supervisor_observation,
        coordinator_observation,
        set_aside_files,
        notes,
        all_scanned_ids,
        ids_seqs,
        &provisional_map,
    )
    .map(publication::PublicationOutcome::into_closure)
}

#[cfg(target_os = "linux")]
fn find_live_process_by_launch_env(
    journal: &std::path::Path,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Option<AdmissionIdentity> {
    use std::os::unix::fs::MetadataExt;
    let own_uid = nix::unistd::getuid().as_raw();
    let proc_dir = fs::read_dir("/proc").ok()?;
    for entry in proc_dir.flatten() {
        let name = entry.file_name();
        let name_str = name.to_string_lossy();
        let Ok(pid) = name_str.parse::<u32>() else {
            continue;
        };
        if pid == 0 {
            continue;
        }
        let proc_path = entry.path();
        let Ok(meta) = fs::metadata(&proc_path) else {
            continue;
        };
        if meta.uid() != own_uid {
            continue;
        }
        let environ_path = proc_path.join("environ");
        let Ok(environ_bytes) = fs::read(&environ_path) else {
            continue;
        };
        let mut has_journal = false;
        let mut has_launch_id = false;
        for var in environ_bytes.split(|&b| b == 0) {
            if var.is_empty() {
                continue;
            }
            if let Ok(var_str) = std::str::from_utf8(var)
                && let Some((k, v)) = var_str.split_once('=')
            {
                if k == "SOLSTONE_JOURNAL" && std::path::Path::new(v) == journal {
                    has_journal = true;
                } else if k == "SOL_PARENT_LOSS_LAUNCH_ID" && v == launch_id {
                    has_launch_id = true;
                }
            }
        }
        if has_journal && has_launch_id {
            let source = SystemProcessInstanceSource;
            if let InspectResult::Present { instance, uid, .. } = source.inspect(pid) {
                return Some(AdmissionIdentity {
                    generation,
                    launch_id: launch_id.to_string(),
                    instance,
                    uid,
                    parent_launch_id: None,
                });
            }
        }
    }
    None
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
        write_parent_loss_admission_spawn_identity,
    };
    use crate::lifecycle::parent_loss_ledger::{ActiveGeneration, CoordinatorLease};
    use crate::lifecycle::{
        HEARTBEAT_SCHEMA_V2, HeartbeatV2, RunId, WriterId, v2_heartbeat_filename,
    };
    use crate::process::{CensusRow, ExecutionState, InspectResult, InstanceCensus, ProcessBirth};

    fn instance(pid: u32, birth: u64) -> ProcessInstance {
        ProcessInstance {
            pid,
            birth: ProcessBirth::linux(birth, 1, 100),
        }
    }

    #[derive(Default)]
    struct FakeTable {
        live: HashMap<u32, (ProcessInstance, u32)>,
        unverifiable: HashSet<u32>,
        incomplete_census: bool,
        owners: HashMap<u32, ProcessOwner>,
        observed: Vec<ProcessInstance>,
        observed_pids: Vec<u32>,
    }

    #[derive(Clone, Default)]
    struct FakeSource(Arc<Mutex<FakeTable>>);

    impl ProcessInstanceSource for FakeSource {
        fn inspect(&self, pid: u32) -> InspectResult {
            let mut table = self.0.lock().expect("fake table");
            table.observed_pids.push(pid);
            let live_inst = table.live.get(&pid).map(|(i, _)| *i);
            if let Some(instance) = live_inst {
                table.observed.push(instance);
            }
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
            let table = self.0.lock().expect("fake table");
            let rows = table
                .live
                .values()
                .map(|(instance, uid)| CensusRow {
                    instance: *instance,
                    uid: *uid,
                    ppid: 1,
                    pgid: 1,
                    execution: ExecutionState::Running,
                })
                .collect();
            if table.incomplete_census {
                InstanceCensus::Incomplete(rows)
            } else {
                InstanceCensus::Complete(rows)
            }
        }
    }

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
            incomplete_census: false,
            owners: HashMap::new(),
            observed: Vec::new(),
            observed_pids: Vec::new(),
        }));
        let retirer = FakeRetirer {
            table: Arc::clone(&table),
            signals: Mutex::new(Vec::new()),
            obeys_terminate: obeys_terminate.iter().copied().collect(),
            immortal: immortal.iter().copied().collect(),
        };
        (FakeSource(table), retirer)
    }

    fn fake_with_owners(
        live: &[(ProcessInstance, u32)],
        obeys_terminate: &[u32],
        immortal: &[u32],
        owners: &[(u32, ProcessOwner)],
    ) -> (FakeSource, FakeRetirer) {
        let table = Arc::new(Mutex::new(FakeTable {
            live: live.iter().map(|(i, uid)| (i.pid, (*i, *uid))).collect(),
            unverifiable: HashSet::new(),
            incomplete_census: false,
            owners: owners.iter().copied().collect(),
            observed: Vec::new(),
            observed_pids: Vec::new(),
        }));
        let retirer = FakeRetirer {
            table: Arc::clone(&table),
            signals: Mutex::new(Vec::new()),
            obeys_terminate: obeys_terminate.iter().copied().collect(),
            immortal: immortal.iter().copied().collect(),
        };
        (FakeSource(table), retirer)
    }

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

    fn acquire_lease(ledger: &ParentLossLedger) -> CoordinatorLease {
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
        let lease = acquire_lease(&ledger);

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
    }

    #[test]
    fn a_live_coordinator_still_refuses_the_closer() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let coord_instance = ledger
            .record(active.generation)
            .expect("record")
            .expect("record present")
            .coordinator
            .expect("coordinator identity");
        let (source, retirer) = fake(&[(coord_instance, 501)], &[], &[]);
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("a live coordinator must refuse closure");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::ActiveCoordinator)
        ));
        assert!(
            !ledger
                .closure_recovery_batches_dir(active.generation)
                .exists()
                || fs::read_dir(ledger.closure_recovery_batches_dir(active.generation))
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true),
            "no recovery writes on veto"
        );
    }

    #[test]
    fn a_live_supervisor_still_refuses_the_closer() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let sup_instance = ledger
            .record(active.generation)
            .expect("record")
            .expect("record present")
            .supervisor;
        let (source, retirer) = fake(&[(sup_instance, 501)], &[], &[]);
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("a live supervisor must refuse closure");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::SupervisorLive)
        ));
        assert!(
            !ledger
                .closure_recovery_batches_dir(active.generation)
                .exists()
                || fs::read_dir(ledger.closure_recovery_batches_dir(active.generation))
                    .map(|mut d| d.next().is_none())
                    .unwrap_or(true),
            "no recovery writes on veto"
        );
    }

    #[test]
    fn an_admission_that_outlives_the_deadline_refuses_this_start_and_leaves_the_generation_open() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let stubborn = instance(5000, 50);
        admit(
            directory.path(),
            active.generation,
            "stubborn",
            Some(HostedServiceKind::Sense),
            Some(stubborn),
        );
        let (source, retirer) = fake(&[(stubborn, 501)], &[], &[stubborn.pid]);
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_millis(50)),
            )
            .expect_err("timeout must refuse");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::CoordinatorNotLive)
        ));
        assert!(ledger.active_path().is_file());
    }

    #[test]
    fn a_child_that_obeys_sigterm_is_not_escalated_and_the_signals_are_recorded() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let obedient = instance(6000, 60);
        admit(
            directory.path(),
            active.generation,
            "obedient",
            Some(HostedServiceKind::Sense),
            Some(obedient),
        );
        let (source, retirer) = fake(&[(obedient, 501)], &[obedient.pid], &[]);
        let lease = acquire_lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closure succeeds");

        let signals = retirer.signals.lock().expect("signals");
        assert_eq!(signals.as_slice(), &[(obedient.pid, "term")]);
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(
            closure.admissions[0].finding,
            AdmissionFinding::Retired { escalated: false }
        );
    }

    #[test]
    fn incomplete_descendant_coverage_refuses_without_retiring_the_root() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let parent = instance(7000, 70);
        admit(
            directory.path(),
            active.generation,
            "parent",
            Some(HostedServiceKind::Sense),
            Some(parent),
        );
        let (source, retirer) = fake(&[(parent, 501)], &[parent.pid], &[]);
        source.0.lock().expect("fake").incomplete_census = true;
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("incomplete census must refuse");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
            )
        ));
        assert!(retirer.signals.lock().expect("signals").is_empty());
    }

    #[cfg(feature = "full-tests")]
    #[test]
    fn an_uninspectable_pid_is_settled_by_its_owner_or_refuses() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let gone = instance(8001, 80);
        let foreign = instance(8002, 80);
        let mine = instance(8003, 80);
        admit(
            directory.path(),
            active.generation,
            "gone",
            Some(HostedServiceKind::Sense),
            Some(gone),
        );
        admit(
            directory.path(),
            active.generation,
            "foreign",
            Some(HostedServiceKind::Sense),
            Some(foreign),
        );
        admit(
            directory.path(),
            active.generation,
            "mine",
            Some(HostedServiceKind::Sense),
            Some(mine),
        );

        let (source, retirer) = fake(&[], &[], &[]);
        {
            let mut table = source.0.lock().expect("table");
            table.unverifiable.insert(gone.pid);
            table.unverifiable.insert(foreign.pid);
            table.unverifiable.insert(mine.pid);
            table.owners.insert(gone.pid, ProcessOwner::Absent);
            table.owners.insert(foreign.pid, ProcessOwner::Uid(9999));
            table.owners.insert(mine.pid, ProcessOwner::Uid(501));
        }
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("mine staying unverifiable must refuse");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
            )
        ));
    }

    #[test]
    fn the_pointer_is_sealed_before_the_admissions_are_scanned() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
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
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(
                active.generation,
                "unacknowledged",
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("write intent");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

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
    }

    #[test]
    fn a_drop_torn_by_the_kill_closes_and_retires_the_live_child() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let generation = active.generation;
        let torn = |launch_id: &str, drop: &str| {
            let path = ledger
                .generation_path(generation)
                .join("admissions")
                .join(launch_id)
                .join(drop);
            fs::create_dir_all(path.parent().expect("admission directory")).expect("directory");
            fs::File::create(path).expect("torn drop");
        };
        let identity = |launch_id: &str, child: ProcessInstance| AdmissionIdentity {
            generation,
            launch_id: launch_id.to_owned(),
            instance: child,
            uid: 501,
            parent_launch_id: None,
        };
        let intent = |launch_id: &str| {
            write_parent_loss_admission_intent(
                directory.path(),
                &AdmissionIntent::new(generation, launch_id, Some(HostedServiceKind::Sense), None),
            )
            .expect("intent");
        };
        // Killed while writing the spawn identity; the child acknowledged.
        let spawn_torn = instance(4301, 43);
        intent("spawn-torn");
        torn("spawn-torn", "spawn-identity.json");
        acknowledge_parent_loss_admission(directory.path(), identity("spawn-torn", spawn_torn))
            .expect("ack");
        // The child was writing its acknowledgement.
        let ack_torn = instance(4302, 43);
        intent("ack-torn");
        write_parent_loss_admission_spawn_identity(
            directory.path(),
            &identity("ack-torn", ack_torn),
        )
        .expect("spawn identity");
        torn("ack-torn", "acknowledgement.json");
        // Killed while writing the result.
        let result_torn = instance(4303, 43);
        intent("result-torn");
        write_parent_loss_admission_spawn_identity(
            directory.path(),
            &identity("result-torn", result_torn),
        )
        .expect("spawn identity");
        acknowledge_parent_loss_admission(directory.path(), identity("result-torn", result_torn))
            .expect("ack");
        torn("result-torn", "result.json");
        // Killed while writing the intent, before any child existed.
        torn("intent-torn", "intent.json");
        // The result-torn child has since exited; the other two outlive the kill.
        let (source, retirer) = fake(&[(spawn_torn, 501), (ack_torn, 501)], &[4301, 4302], &[]);
        let lease = acquire_lease(&ledger);

        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("a drop torn by the kill must not brick the journal");

        assert_eq!(successor.generation, generation + 1);
        let mut signals = retirer.signals.lock().expect("signals").clone();
        signals.sort_unstable();
        assert_eq!(
            signals,
            vec![(spawn_torn.pid, "term"), (ack_torn.pid, "term")],
            "each child that outlived the kill is retired by the identity that was written"
        );
        let closure = closure_of(&ledger, generation);
        let finding = |launch_id: &str| {
            closure
                .admissions
                .iter()
                .find(|admission| admission.launch_id == launch_id)
                .map(|admission| admission.finding.clone())
                .expect("the closure names the admission")
        };
        assert!(matches!(
            finding("spawn-torn"),
            AdmissionFinding::Retired { .. }
        ));
        assert!(matches!(
            finding("ack-torn"),
            AdmissionFinding::Retired { .. }
        ));
        assert_eq!(finding("result-torn"), AdmissionFinding::Exited);
    }

    #[test]
    fn a_missing_record_is_created_and_closed() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let active = ledger
            .reserve_generation(instance(10, 1), [])
            .expect("reservation without a record");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

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
        fs::write(ledger.record_path(active.generation), b"not valid json").expect("corrupt");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        let closure = closure_of(&ledger, active.generation);
        assert!(!closure.set_aside.is_empty());
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
            Some(HostedServiceKind::Sense),
            Some(live),
        );
        std::fs::write(ledger.active_path(), b"{ this is not json").expect("corrupt the pointer");
        let (source, retirer) = fake(&[(live, 501)], &[live.pid], &[]);
        let lease = acquire_lease(&ledger);

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
        let (ledger, active) = open_generation(&directory);
        let writer_id = WriterId::parse("0123456789abcdef0123456789abcdef").expect("writer ID");
        let run_id = RunId::parse("fedcba9876543210fedcba9876543210").expect("run ID");
        let heartbeat_filename = v2_heartbeat_filename(&writer_id, &run_id);
        let sync_dir = directory.path().join("health").join("sync");
        fs::create_dir_all(&sync_dir).expect("sync dir");
        let heartbeat_path = sync_dir.join(&heartbeat_filename);
        let supervisor = instance(10, 1);
        let record = ParentLossGenerationRecord {
            schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
            generation: active.generation,
            supervisor,
            coordinator: Some(instance(20, 2)),
            sealed_ledger_digest: None,
            terminal: None,
            supervisor_heartbeat: Some(heartbeat_filename),
            closure: None,
        };
        write_json(
            ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .expect("record");
        let heartbeat = HeartbeatV2::new(
            writer_id,
            run_id,
            "lost-supervisor".to_owned(),
            supervisor.pid,
            "1.0".to_owned(),
            "test".to_owned(),
            15,
            directory.path().display().to_string(),
        );
        assert_eq!(heartbeat.schema, HEARTBEAT_SCHEMA_V2);
        write_json(&heartbeat_path, &heartbeat, json_options()).expect("heartbeat");
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.heartbeat, HeartbeatRetirement::Removed);
        assert!(!heartbeat_path.exists());
    }

    #[test]
    fn a_record_without_a_heartbeat_name_records_that_nothing_was_retired() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.heartbeat, HeartbeatRetirement::NotRecorded);
    }

    #[test]
    fn a_sealed_unresolved_generation_is_advanced_past_with_its_admissions_retired() {
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
        let lease = acquire_lease(&ledger);

        ledger
            .mark_sealed_for_closure(active.generation)
            .expect("seal");

        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closed");

        assert_eq!(successor.generation, active.generation + 1);
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
        let lease = acquire_lease(&ledger);

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

    #[test]
    fn the_coordinator_lease_is_exclusive_while_held() {
        let directory = TempDir::new().expect("temporary root");
        let ledger = ParentLossLedger::open(directory.path()).expect("ledger");
        let lease = ledger
            .acquire_coordinator_lease()
            .expect("lease")
            .expect("free");
        let second = ledger.acquire_coordinator_lease().expect("second check");
        assert!(second.is_none());
        drop(lease);
        let third = ledger.acquire_coordinator_lease().expect("third check");
        assert!(third.is_some());
    }

    #[test]
    fn recovery_checkpoint_budget_refusal_is_non_advancing_reason() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        let expired_authority = ClosingAuthority {
            closed_by: instance(30, 3),
            source: &source,
            retirer: &retirer,
            deadline: Instant::now() - Duration::from_millis(10),
        };

        let error = ledger
            .reserve_generation_closing_abandoned(instance(11, 3), [], &lease, &expired_authority)
            .expect_err("expired deadline must refuse");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::CoordinatorNotLive)
        ));
        let active_after = ledger.active_generation().expect("read").expect("active");
        assert_eq!(active_after.generation, active.generation);
    }

    #[test]
    fn digest_mismatch_with_omitted_or_changed_uid_or_instance_or_parent_does_not_authorize() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "test-launch-1";
        let inst = instance(4001, 100);
        admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(inst),
        );

        let derived = read_derived_admission_drops(&ledger, active.generation, launch_id)
            .expect("derived drops");

        // Row with omitted uid:
        let invalid_row_no_uid = CompletedBatchRow {
            sources: None,
            launch_id: launch_id.to_string(),
            service: derived.service,
            instance: Some(inst),
            uid: None,
            parent_launch_id: None,
            finding: AdmissionFinding::Exited,
            parsed_result_state: derived.parsed_result_state.clone(),
            intent_digest: derived.digests.intent.clone(),
            result_digest: derived.digests.result.clone(),
            ack_digest: derived.digests.ack.clone(),
            spawn_identity_digest: derived.digests.spawn_identity.clone(),
            binding_digest: "some-digest".to_string(),
        };
        assert!(!completed_row_is_authorized(
            &ledger,
            active.generation,
            &invalid_row_no_uid
        ));

        // Row with changed instance:
        let changed_inst = instance(4002, 200);
        let Ok(digest_changed) = compute_batch_row_binding_digest(
            launch_id,
            derived.service,
            Some(changed_inst),
            derived.uid,
            None,
            AdmissionFinding::Exited,
            derived.parsed_result_state.as_deref(),
            derived.digests.intent.as_deref(),
            derived.digests.result.as_deref(),
            derived.digests.ack.as_deref(),
            derived.digests.spawn_identity.as_deref(),
        ) else {
            panic!("compute digest");
        };
        let row_changed_inst = CompletedBatchRow {
            sources: None,
            launch_id: launch_id.to_string(),
            service: derived.service,
            instance: Some(changed_inst),
            uid: derived.uid,
            parent_launch_id: None,
            finding: AdmissionFinding::Exited,
            parsed_result_state: derived.parsed_result_state.clone(),
            intent_digest: derived.digests.intent.clone(),
            result_digest: derived.digests.result.clone(),
            ack_digest: derived.digests.ack.clone(),
            spawn_identity_digest: derived.digests.spawn_identity.clone(),
            binding_digest: digest_changed,
        };
        assert!(!completed_row_is_authorized(
            &ledger,
            active.generation,
            &row_changed_inst
        ));

        // Plant batch with invalid row:
        let batch_file = RecoveryBatchFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Batch,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            entries: vec![invalid_row_no_uid],
        };
        let b_path = ledger.closure_recovery_batch_path(active.generation, 1);
        fs::create_dir_all(b_path.parent().unwrap()).unwrap();
        write_json(&b_path, &batch_file, json_options()).unwrap();

        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closure succeeds rebuilding from drops");

        let closure = closure_of(&ledger, active.generation);
        assert!(!closure.set_aside.is_empty(), "invalid batch was set aside");
        assert_eq!(closure.admissions.len(), 1);
        assert_eq!(closure.admissions[0].finding, AdmissionFinding::Exited);
        assert_eq!(closure.admissions[0].instance, Some(inst));
    }

    #[test]
    fn outside_sentinel_launch_id_rejected_before_path_join() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);

        let outside_file = directory.path().join("sentinel.json");
        fs::write(&outside_file, b"outside sentinel").unwrap();

        let invalid_ids = [
            "../sentinel",
            "..",
            "foo/bar",
            "foo\\bar",
            "",
            "/etc/passwd",
        ];
        for bad_id in invalid_ids {
            let err = read_derived_admission_drops(&ledger, active.generation, bad_id);
            assert!(matches!(
                err,
                Err(ParentLossLedgerError::RecoveryRequired(
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
                ))
            ));

            let row = CompletedBatchRow {
                sources: None,
                launch_id: bad_id.to_string(),
                service: None,
                instance: None,
                uid: None,
                parent_launch_id: None,
                finding: AdmissionFinding::NeverAcknowledged,
                parsed_result_state: None,
                intent_digest: None,
                result_digest: None,
                ack_digest: None,
                spawn_identity_digest: None,
                binding_digest: String::new(),
            };
            assert!(!completed_row_is_authorized(
                &ledger,
                active.generation,
                &row
            ));
        }
    }

    #[test]
    fn a_failed_nested_launch_closes_with_its_original_parent_provenance() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let launch_id = "failed-child";
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(
                active.generation,
                launch_id,
                None,
                Some("parent-launch".into()),
            ),
        )
        .unwrap();
        write_parent_loss_admission_result(
            directory.path(),
            active.generation,
            launch_id,
            &AdmissionResult {
                schema: 1,
                identity: None,
                state: AdmissionResultState::SpawnFailed {
                    detail: "spawn failed".into(),
                },
            },
        )
        .unwrap();
        let before = fs::read(
            ledger
                .generation_path(active.generation)
                .join("admissions/failed-child/intent.json"),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        let next = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("a failed nested launch must not permanently refuse publication");
        assert!(next.generation > active.generation);
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.admissions.len(), 1);
        assert_eq!(closure.admissions[0].finding, AdmissionFinding::SpawnFailed);
        let batch: RecoveryBatchFile = serde_json::from_slice(
            &fs::read(ledger.closure_recovery_batch_path(active.generation, 1)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            batch.entries[0].parent_launch_id.as_deref(),
            Some("parent-launch")
        );
        assert_eq!(
            fs::read(
                ledger
                    .generation_path(active.generation)
                    .join("admissions/failed-child/intent.json")
            )
            .unwrap(),
            before
        );
        assert!(retirer.signals.lock().unwrap().is_empty());
    }

    #[test]
    fn spawn_failed_with_invalid_parsed_result_state_does_not_authorize() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "failed-launch";

        let invalid_spawn_failed = CompletedBatchRow {
            sources: None,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            instance: None,
            uid: None,
            parent_launch_id: None,
            finding: AdmissionFinding::SpawnFailed,
            parsed_result_state: Some("other_state".to_string()),
            intent_digest: None,
            result_digest: None,
            ack_digest: None,
            spawn_identity_digest: None,
            binding_digest: "digest".to_string(),
        };
        assert!(!completed_row_is_authorized(
            &ledger,
            active.generation,
            &invalid_spawn_failed
        ));

        // Plant intent and spawn_failed result drops
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(
                active.generation,
                launch_id,
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("write intent");

        let result = AdmissionResult {
            schema: 1,
            identity: None,
            state: AdmissionResultState::SpawnFailed {
                detail: "failed to spawn".to_string(),
            },
        };
        write_parent_loss_admission_result(directory.path(), active.generation, launch_id, &result)
            .expect("write result");

        let derived = read_derived_admission_drops(&ledger, active.generation, launch_id)
            .expect("derived drops");
        assert!(derived.digests.result.is_some());

        let valid_digest = compute_batch_row_binding_digest(
            launch_id,
            Some(HostedServiceKind::Sense),
            None,
            None,
            None,
            AdmissionFinding::SpawnFailed,
            Some("spawn_failed"),
            derived.digests.intent.as_deref(),
            derived.digests.result.as_deref(),
            None,
            None,
        )
        .unwrap();
        let valid_spawn_failed = CompletedBatchRow {
            sources: None,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            instance: None,
            uid: None,
            parent_launch_id: None,
            finding: AdmissionFinding::SpawnFailed,
            parsed_result_state: Some("spawn_failed".to_string()),
            intent_digest: derived.digests.intent,
            result_digest: derived.digests.result,
            ack_digest: None,
            spawn_identity_digest: None,
            binding_digest: valid_digest,
        };
        assert!(completed_row_is_authorized(
            &ledger,
            active.generation,
            &valid_spawn_failed
        ));
    }

    #[test]
    fn escalation_target_unauthorized_by_drops_is_not_signalled_and_invalid_launch_id_never_joins()
    {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "admitted-target";
        let real_inst = instance(5001, 100);
        admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(real_inst),
        );

        let unauth_inst = instance(5002, 200);
        let live_file = LiveRecoveryFile {
            sources: None,
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            root: unauth_inst,
            uid: 501,
            parent_launch_id: None,
            descendants: Vec::new(),
            escalate_at_unix_ms: 0,
            reap_deadline_unix_ms: None,
            term_requested: false,
            escalated: false,
            binding_digest: compute_live_binding_digest(unauth_inst, 501, &[]).unwrap(),
        };
        let live_path = ledger.closure_recovery_live_path(active.generation, launch_id);
        fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        write_json(&live_path, &live_file, json_options()).unwrap();

        let (source, retirer) = fake(&[(unauth_inst, 501)], &[], &[]);
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_millis(50)),
            )
            .expect_err("root mismatch with dead real instance refuses");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
            )
        ));
        assert!(
            retirer.signals.lock().unwrap().is_empty(),
            "unauthorized instance must not be signalled"
        );

        let bad_dir = directory
            .path()
            .join("health/parent-loss/generations")
            .join(format!("{}", active.generation))
            .join("admissions")
            .join("invalid..launch/id");
        fs::create_dir_all(&bad_dir).unwrap();
        let (source2, retirer2) = fake(&[], &[], &[]);
        let error2 = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source2, &retirer2, Duration::from_millis(50)),
            )
            .expect_err("invalid launch id must refuse");
        assert!(matches!(
            error2,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
            )
        ));
    }

    #[test]
    fn malformed_duplicate_and_missing_batches_do_not_certify_omitted_ids_and_wrong_gen_or_phase_set_aside()
     {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "dup-id";
        let inst = instance(7001, 100);
        admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(inst),
        );

        let derived = read_derived_admission_drops(&ledger, active.generation, launch_id).unwrap();
        let digest = compute_batch_row_binding_digest(
            launch_id,
            derived.service,
            Some(inst),
            derived.uid,
            None,
            AdmissionFinding::Exited,
            derived.parsed_result_state.as_deref(),
            derived.digests.intent.as_deref(),
            derived.digests.result.as_deref(),
            derived.digests.ack.as_deref(),
            derived.digests.spawn_identity.as_deref(),
        )
        .unwrap();
        let row = CompletedBatchRow {
            sources: None,
            launch_id: launch_id.to_string(),
            service: derived.service,
            instance: Some(inst),
            uid: derived.uid,
            parent_launch_id: None,
            finding: AdmissionFinding::Exited,
            parsed_result_state: derived.parsed_result_state,
            intent_digest: derived.digests.intent,
            result_digest: derived.digests.result,
            ack_digest: derived.digests.ack,
            spawn_identity_digest: derived.digests.spawn_identity,
            binding_digest: digest,
        };

        // Plant batch 1:
        let batch1 = RecoveryBatchFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Batch,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            entries: vec![row.clone()],
        };
        let b1_path = ledger.closure_recovery_batch_path(active.generation, 1);
        fs::create_dir_all(b1_path.parent().unwrap()).unwrap();
        write_json(&b1_path, &batch1, json_options()).unwrap();
        let ids1 = RecoveryIdsFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Ids,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            ids: vec![launch_id.to_string()],
        };
        let i1_path = ledger.closure_recovery_ids_path(active.generation, 1);
        fs::create_dir_all(i1_path.parent().unwrap()).unwrap();
        write_json(&i1_path, &ids1, json_options()).unwrap();

        // Plant batch 2 with duplicate launch_id:
        let batch2 = RecoveryBatchFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Batch,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 2,
            entries: vec![row],
        };
        let b2_path = ledger.closure_recovery_batch_path(active.generation, 2);
        write_json(&b2_path, &batch2, json_options()).unwrap();
        let ids2 = RecoveryIdsFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Ids,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 2,
            ids: vec![launch_id.to_string()],
        };
        let i2_path = ledger.closure_recovery_ids_path(active.generation, 2);
        write_json(&i2_path, &ids2, json_options()).unwrap();

        // Plant batch 3 with wrong generation:
        let batch3 = RecoveryBatchFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Batch,
            generation: active.generation + 99,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 3,
            entries: Vec::new(),
        };
        let b3_path = ledger.closure_recovery_batch_path(active.generation, 3);
        write_json(&b3_path, &batch3, json_options()).unwrap();

        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("closure succeeds");

        let closure = closure_of(&ledger, active.generation);
        assert!(
            !closure.set_aside.is_empty(),
            "conflicting/wrong batches were set aside"
        );
    }

    #[test]
    fn ids_naming_a_live_admission_do_not_certify_when_the_batch_holds_another() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let id_a = "admission-a";
        let id_b = "admission-b";
        let inst_a = instance(5001, 100);
        let inst_b = instance(5002, 200);
        admit(
            directory.path(),
            active.generation,
            id_a,
            Some(HostedServiceKind::Sense),
            Some(inst_a),
        );
        admit(
            directory.path(),
            active.generation,
            id_b,
            Some(HostedServiceKind::Sense),
            Some(inst_b),
        );

        let (source, retirer) = fake(&[(inst_b, 501)], &[], &[]);
        let lease = acquire_lease(&ledger);

        let derived_a = read_derived_admission_drops(&ledger, active.generation, id_a).unwrap();
        let digest_a = compute_batch_row_binding_digest(
            id_a,
            derived_a.service,
            Some(inst_a),
            derived_a.uid,
            None,
            AdmissionFinding::Exited,
            derived_a.parsed_result_state.as_deref(),
            derived_a.digests.intent.as_deref(),
            derived_a.digests.result.as_deref(),
            derived_a.digests.ack.as_deref(),
            derived_a.digests.spawn_identity.as_deref(),
        )
        .unwrap();
        let row_a = CompletedBatchRow {
            sources: None,
            launch_id: id_a.to_string(),
            service: derived_a.service,
            instance: Some(inst_a),
            uid: derived_a.uid,
            parent_launch_id: None,
            finding: AdmissionFinding::Exited,
            parsed_result_state: derived_a.parsed_result_state,
            intent_digest: derived_a.digests.intent,
            result_digest: derived_a.digests.result,
            ack_digest: derived_a.digests.ack,
            spawn_identity_digest: derived_a.digests.spawn_identity,
            binding_digest: digest_a,
        };

        let batch1 = RecoveryBatchFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Batch,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            entries: vec![row_a],
        };
        let b1_path = ledger.closure_recovery_batch_path(active.generation, 1);
        fs::create_dir_all(b1_path.parent().unwrap()).unwrap();
        write_json(&b1_path, &batch1, json_options()).unwrap();

        let ids1 = RecoveryIdsFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Ids,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            ids: vec![id_b.to_string()],
        };
        let i1_path = ledger.closure_recovery_ids_path(active.generation, 1);
        fs::create_dir_all(i1_path.parent().unwrap()).unwrap();
        write_json(&i1_path, &ids1, json_options()).unwrap();

        let batch_bytes = fs::read(&b1_path).unwrap();
        let receipt1 = RecoveryBatchReceipt {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Receipt,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            batch_bytes_digest: digest_bytes(&batch_bytes),
            ids: vec![id_b.to_string()],
        };
        let r1_path = ledger.closure_recovery_receipt_path(active.generation, 1);
        fs::create_dir_all(r1_path.parent().unwrap()).unwrap();
        write_json(&r1_path, &receipt1, json_options()).unwrap();

        let _ = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("mismatched batch and live admission must fail closure");

        let record = ledger
            .record(active.generation)
            .expect("record read")
            .expect("record exists");
        assert!(record.closure.is_none(), "closure record must remain None");
        assert_eq!(
            ledger.active_generation().unwrap().unwrap().generation,
            active.generation,
            "successor pointer stays on this generation"
        );
        assert!(
            admission_directory(&ledger, active.generation)
                .join(id_b)
                .is_dir(),
            "B's admission directory remains present"
        );
    }

    #[test]
    fn live_descendant_owned_by_another_user_refuses_closure_without_signalling() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "alien-descendant";
        let root_inst = instance(9901, 100);
        let desc_inst = instance(9902, 200);

        admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(root_inst),
        );

        let descendants = vec![LiveDescendantIdentity {
            instance: desc_inst,
            uid: 9,
            ppid: root_inst.pid,
            pgid: root_inst.pid,
            escalate_at_unix_ms: 0,
            reap_deadline_unix_ms: None,
            term_requested: false,
            escalated: false,
        }];
        let live_file = LiveRecoveryFile {
            sources: None,
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            root: root_inst,
            uid: 501,
            parent_launch_id: None,
            descendants: descendants.clone(),
            escalate_at_unix_ms: 0,
            reap_deadline_unix_ms: None,
            term_requested: true,
            escalated: false,
            binding_digest: compute_live_binding_digest(root_inst, 501, &descendants).unwrap(),
        };
        let live_path = ledger.closure_recovery_live_path(active.generation, launch_id);
        fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        write_json(&live_path, &live_file, json_options()).unwrap();

        let obligations = HashMap::from([(launch_id.to_string(), live_file.clone())]);
        let ob_file = CensusObligationFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            obligations: obligations.clone(),
            binding_digest: compute_census_obligation_digest(&obligations).unwrap(),
        };
        write_json(
            ledger.census_obligation_path(active.generation),
            &ob_file,
            json_options(),
        )
        .unwrap();

        // Root is exited (not in live), descendant is live with stored UID 501, but owner is UID 9:
        let (source, retirer) = fake_with_owners(
            &[(desc_inst, 9)],
            &[],
            &[desc_inst.pid],
            &[(desc_inst.pid, ProcessOwner::Uid(9))],
        );
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("alien descendant must refuse closure");

        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionLive
            )
        ));
        assert!(
            !retirer
                .signals
                .lock()
                .unwrap()
                .iter()
                .any(|(p, _)| *p == desc_inst.pid),
            "must not signal descendant owned by another uid"
        );
        let record = ledger
            .record(active.generation)
            .expect("record read")
            .expect("record exists");
        assert!(
            record.closure.is_none(),
            "closure record must be absent on refusal"
        );
    }

    #[test]
    fn provisional_row_gains_acknowledgement_between_attempts_retired_under_seal() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "late-ack-launch";
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(
                active.generation,
                launch_id,
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .expect("intent");

        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        // First attempt with PROVISIONAL_STOP_FAULT flag: provisional file written
        PROVISIONAL_STOP_FAULT.with(|f| f.set(true));
        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("provisional fault returns CoordinatorNotLive");
        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::CoordinatorNotLive)
        ));

        let prov_path = ledger.closure_recovery_provisional_path(active.generation, launch_id);
        assert!(prov_path.is_file(), "provisional file is on disk");
        let active_after = ledger.active_generation().expect("read").expect("active");
        assert_eq!(active_after.generation, active.generation);

        PROVISIONAL_STOP_FAULT.with(|f| f.set(false));

        // Late acknowledgement added:
        let ack_inst = instance(6001, 100);
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.to_string(),
            instance: ack_inst,
            uid: 501,
            parent_launch_id: None,
        };
        acknowledge_parent_loss_admission(directory.path(), identity).expect("ack");

        // Second attempt with flag false and adequate budget:
        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("second attempt closes");

        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.admissions.len(), 1);
        assert_eq!(closure.admissions[0].finding, AdmissionFinding::Exited);
        assert_eq!(closure.admissions[0].instance, Some(ack_inst));
        assert!(
            !prov_path.exists(),
            "provisional file removed after completion"
        );
    }

    #[test]
    fn a_coherently_rehashed_receipt_cannot_change_the_admitted_uid() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "cached-uid";
        let admitted = admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(instance(8101, 100)),
        )
        .expect("admitted identity");
        let source_directory = admission_directory(&ledger, active.generation).join(launch_id);
        let source_bytes: Vec<_> = ["intent.json", "result.json", "acknowledgement.json"]
            .into_iter()
            .map(|name| {
                (
                    name,
                    fs::read(source_directory.join(name)).expect("source drop"),
                )
            })
            .collect();
        let derived = read_derived_admission_drops(&ledger, active.generation, launch_id)
            .expect("derive original evidence");
        let forged_uid = Some(admitted.uid + 1);
        let binding_digest = compute_batch_row_binding_digest(
            launch_id,
            derived.service,
            derived.instance,
            forged_uid,
            derived.parent_launch_id.as_deref(),
            AdmissionFinding::Exited,
            derived.parsed_result_state.as_deref(),
            derived.digests.intent.as_deref(),
            derived.digests.result.as_deref(),
            derived.digests.ack.as_deref(),
            derived.digests.spawn_identity.as_deref(),
        )
        .expect("coherent derived checksum");
        let mut rows = vec![CompletedBatchRow {
            sources: Some(derived.sources.clone()),
            launch_id: launch_id.to_owned(),
            service: derived.service,
            instance: derived.instance,
            uid: forged_uid,
            parent_launch_id: derived.parent_launch_id,
            finding: AdmissionFinding::Exited,
            parsed_result_state: derived.parsed_result_state,
            intent_digest: derived.digests.intent,
            result_digest: derived.digests.result,
            ack_digest: derived.digests.ack,
            spawn_identity_digest: derived.digests.spawn_identity,
            binding_digest,
        }];
        fs::create_dir_all(ledger.closure_recovery_batches_dir(active.generation))
            .expect("batches directory");
        fs::create_dir_all(ledger.closure_recovery_ids_dir(active.generation))
            .expect("index directory");
        flush_batch_and_ids(
            &ledger,
            active.generation,
            1,
            &mut rows,
            &mut HashSet::new(),
        )
        .expect("seed coherent batch, index and receipt");

        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        let refused = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("a coherent cache cannot authorize allocation");
        assert!(matches!(
            refused,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::CoordinatorNotLive)
        ));
        assert_eq!(
            ledger.active_generation().unwrap().unwrap().generation,
            active.generation
        );
        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("next start recovers from preserved original evidence");

        for (name, expected) in source_bytes {
            assert_eq!(fs::read(source_directory.join(name)).unwrap(), expected);
        }

        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.admissions.len(), 1);
        assert_eq!(closure.admissions[0].uid, Some(admitted.uid));
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).expect("sealed ledger"),
        )
        .expect("sealed identities");
        assert_eq!(sealed.len(), 1);
        let sealed_identity: AdmissionIdentity =
            serde_json::from_value(sealed[0]["identity"].clone()).expect("sealed identity");
        assert_eq!(sealed_identity, admitted);
    }

    #[test]
    fn recovery_checkpoint_resumes_without_reobserving_valid_cached_rows() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch1 = "launch-1";
        let inst1 = instance(8001, 100);
        admit(
            directory.path(),
            active.generation,
            launch1,
            Some(HostedServiceKind::Sense),
            Some(inst1),
        );
        let launch2 = "launch-2";
        let inst2 = instance(8002, 200);
        admit(
            directory.path(),
            active.generation,
            launch2,
            Some(HostedServiceKind::Sense),
            Some(inst2),
        );

        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        // Pre-populate batch 1 for launch-1
        let derived = read_derived_admission_drops(&ledger, active.generation, launch1).unwrap();
        let digest = compute_batch_row_binding_digest(
            launch1,
            derived.service,
            Some(inst1),
            derived.uid,
            None,
            AdmissionFinding::Exited,
            derived.parsed_result_state.as_deref(),
            derived.digests.intent.as_deref(),
            derived.digests.result.as_deref(),
            derived.digests.ack.as_deref(),
            derived.digests.spawn_identity.as_deref(),
        )
        .unwrap();
        let row1 = CompletedBatchRow {
            sources: Some(derived.sources.clone()),
            launch_id: launch1.to_string(),
            service: derived.service,
            instance: Some(inst1),
            uid: derived.uid,
            parent_launch_id: None,
            finding: AdmissionFinding::Exited,
            parsed_result_state: derived.parsed_result_state,
            intent_digest: derived.digests.intent,
            result_digest: derived.digests.result,
            ack_digest: derived.digests.ack,
            spawn_identity_digest: derived.digests.spawn_identity,
            binding_digest: digest,
        };
        let batch1 = RecoveryBatchFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Batch,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            entries: vec![row1],
        };
        let b1_path = ledger.closure_recovery_batch_path(active.generation, 1);
        fs::create_dir_all(b1_path.parent().unwrap()).unwrap();
        write_json(&b1_path, &batch1, json_options()).unwrap();
        let ids1 = RecoveryIdsFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Ids,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            ids: vec![launch1.to_string()],
        };
        let i1_path = ledger.closure_recovery_ids_path(active.generation, 1);
        fs::create_dir_all(i1_path.parent().unwrap()).unwrap();
        write_json(&i1_path, &ids1, json_options()).unwrap();

        let batch_bytes = fs::read(&b1_path).unwrap();
        let receipt1 = RecoveryBatchReceipt {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Receipt,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            seq: 1,
            batch_bytes_digest: digest_bytes(&batch_bytes),
            ids: vec![launch1.to_string()],
        };
        let r1_path = ledger.closure_recovery_receipt_path(active.generation, 1);
        fs::create_dir_all(r1_path.parent().unwrap()).unwrap();
        write_json(&r1_path, &receipt1, json_options()).unwrap();

        // Mutate on-disk result drop for launch1 to prove it is not re-read
        let result_path = admission_directory(&ledger, active.generation)
            .join(launch1)
            .join("result.json");
        fs::write(&result_path, b"{\"schema\":1,\"mutated\":true}").unwrap();

        source.0.lock().unwrap().observed_pids.clear();

        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(
                    &source,
                    &retirer,
                    PARENT_LOSS_COORDINATOR_RETIREMENT_DEADLINE
                        - PARENT_LOSS_COORDINATOR_TERMINAL_WRITE_BUDGET,
                ),
            )
            .expect("closed");

        let observed = source.0.lock().unwrap().observed_pids.clone();
        assert!(
            !observed.contains(&inst1.pid),
            "completed absent pid must not be re-observed"
        );
        assert!(
            observed.contains(&inst2.pid),
            "uncompleted pid must be observed"
        );

        let record = ledger
            .record(active.generation)
            .expect("record")
            .expect("record exists");
        let closure = record.closure.expect("closure exists");
        let entry1 = closure
            .admissions
            .iter()
            .find(|a| a.launch_id == launch1)
            .expect("launch1 entry");
        assert_eq!(entry1.instance, Some(inst1));
        assert_eq!(entry1.uid, Some(501));
        assert_eq!(entry1.finding, AdmissionFinding::Exited);
    }

    #[test]
    fn retained_census_restores_missing_or_corrupt_live_cache_after_root_exit() {
        for corrupt_cache in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let root = instance(8801, 100);
            let child = instance(8802, 200);
            let launch_id = "lost-live-cache";
            admit(
                directory.path(),
                active.generation,
                launch_id,
                None,
                Some(root),
            );
            let derived =
                read_derived_admission_drops(&ledger, active.generation, launch_id).unwrap();
            let descendants = vec![LiveDescendantIdentity {
                instance: child,
                uid: 501,
                ppid: root.pid,
                pgid: root.pid,
                escalate_at_unix_ms: unix_millis() + LIVE_ESCALATION_GRACE_MS,
                reap_deadline_unix_ms: None,
                term_requested: false,
                escalated: false,
            }];
            let live = LiveRecoveryFile {
                sources: Some(derived.sources),
                schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
                kind: RecoveryArtifactKind::Live,
                generation: active.generation,
                sealed_phase: ParentLossPhase::Sealed,
                launch_id: launch_id.to_owned(),
                service: None,
                root,
                uid: 501,
                parent_launch_id: None,
                binding_digest: compute_live_binding_digest(root, 501, &descendants).unwrap(),
                descendants,
                escalate_at_unix_ms: unix_millis() + LIVE_ESCALATION_GRACE_MS,
                reap_deadline_unix_ms: None,
                term_requested: true,
                escalated: false,
            };
            persist_live(&ledger, active.generation, &live).unwrap();
            let live_path = ledger.closure_recovery_live_path(active.generation, launch_id);
            if corrupt_cache {
                fs::write(&live_path, b"broken cache").unwrap();
            } else {
                fs::remove_file(&live_path).unwrap();
            }
            let (source, retirer) = fake(&[(child, 501)], &[child.pid], &[]);
            let lease = acquire_lease(&ledger);
            ledger
                .reserve_generation_closing_abandoned(
                    instance(11, 3),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2)),
                )
                .unwrap();
            assert_eq!(*retirer.signals.lock().unwrap(), vec![(child.pid, "term")]);
            assert!(
                read_retirement_census(&ledger, active.generation)
                    .unwrap()
                    .obligations
                    .is_empty()
            );
            assert_eq!(
                closure_of(&ledger, active.generation).admissions[0].instance,
                Some(root)
            );
        }
    }

    #[test]
    fn a_descendant_that_loses_observability_or_uid_after_term_cannot_be_discharged_or_killed() {
        struct ChangingRetirer {
            inner: FakeRetirer,
            lose_observability: bool,
        }
        impl AdmissionRetirer for ChangingRetirer {
            fn owner(&self, pid: u32) -> ProcessOwner {
                self.inner.owner(pid)
            }
            fn signal(
                &self,
                target: ProcessInstance,
                signal: SignalKind,
            ) -> Result<(), TerminationError> {
                self.inner.signal(target, signal)?;
                if matches!(signal, SignalKind::Terminate) {
                    let mut table = self.inner.table.lock().unwrap();
                    if self.lose_observability {
                        table.unverifiable.insert(target.pid);
                    } else {
                        table.owners.insert(target.pid, ProcessOwner::Uid(9));
                    }
                }
                Ok(())
            }
        }
        for lose_observability in [true, false] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let root = instance(8810, 100);
            let child = instance(8811, 200);
            let launch_id = "changing-descendant";
            admit(
                directory.path(),
                active.generation,
                launch_id,
                None,
                Some(root),
            );
            let derived =
                read_derived_admission_drops(&ledger, active.generation, launch_id).unwrap();
            let descendants = vec![LiveDescendantIdentity {
                instance: child,
                uid: 501,
                ppid: root.pid,
                pgid: root.pid,
                escalate_at_unix_ms: unix_millis() - 1,
                reap_deadline_unix_ms: None,
                term_requested: true,
                escalated: false,
            }];
            let live = LiveRecoveryFile {
                sources: Some(derived.sources),
                schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
                kind: RecoveryArtifactKind::Live,
                generation: active.generation,
                sealed_phase: ParentLossPhase::Sealed,
                launch_id: launch_id.to_owned(),
                service: None,
                root,
                uid: 501,
                parent_launch_id: None,
                binding_digest: compute_live_binding_digest(root, 501, &descendants).unwrap(),
                descendants,
                escalate_at_unix_ms: unix_millis() - 1,
                reap_deadline_unix_ms: None,
                term_requested: true,
                escalated: false,
            };
            persist_live(&ledger, active.generation, &live).unwrap();
            let (source, inner) = fake(&[(child, 501)], &[], &[child.pid]);
            let retirer = ChangingRetirer {
                inner,
                lose_observability,
            };
            let lease = acquire_lease(&ledger);
            assert!(
                ledger
                    .reserve_generation_closing_abandoned(
                        instance(11, 3),
                        [],
                        &lease,
                        &authority(&source, &retirer, Duration::from_millis(100))
                    )
                    .is_err()
            );
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
            assert!(
                !read_retirement_census(&ledger, active.generation)
                    .unwrap()
                    .obligations
                    .is_empty()
            );
            assert_eq!(
                *retirer.inner.signals.lock().unwrap(),
                vec![(child.pid, "term")]
            );
        }
    }

    #[test]
    fn a_legacy_closed_record_without_a_manifest_is_reaudited_without_changing_its_ledger() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let expected = admit(
            directory.path(),
            active.generation,
            "legacy-closed",
            None,
            Some(instance(8812, 300)),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        let original_ledger = fs::read(ledger.sealed_ledger_path(active.generation)).unwrap();
        fs::remove_file(
            ledger
                .closure_recovery_phase_dir(active.generation)
                .join("publication.json"),
        )
        .unwrap();
        let mut old_pointer = active.clone();
        old_pointer.phase = ParentLossPhase::Sealed;
        write_json(ledger.active_path(), &old_pointer, json_options()).unwrap();
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(
            closure_of(&ledger, active.generation).admissions[0].uid,
            Some(expected.uid)
        );
        assert_eq!(
            fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            original_ledger
        );
    }

    #[test]
    fn late_acknowledgement_invalidates_prepared_publication_before_allocation() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let launch_id = "late-before-publication";
        admit(directory.path(), active.generation, launch_id, None, None);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        PUBLICATION_STOP_FAULT.with(|fault| fault.set(true));
        let first = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(2)),
        );
        PUBLICATION_STOP_FAULT.with(|fault| fault.set(false));
        assert!(matches!(
            first,
            Err(ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::CoordinatorNotLive
            ))
        ));
        assert!(!ledger.sealed_ledger_path(active.generation).exists());
        let expected = admit(
            directory.path(),
            active.generation,
            launch_id,
            None,
            Some(instance(8804, 400)),
        )
        .unwrap();
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.admissions[0].instance, Some(expected.instance));
        assert_eq!(closure.admissions[0].uid, Some(expected.uid));
        let sealed: serde_json::Value = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        let actual: AdmissionIdentity =
            serde_json::from_value(sealed[0]["identity"].clone()).unwrap();
        assert_eq!(actual, expected);
    }

    #[test]
    fn a_complete_large_merge_can_publish_within_its_remaining_slice() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        for ordinal in 0..=RECOVERY_BATCH_LIMIT {
            admit(
                directory.path(),
                active.generation,
                &format!("same-slice-{ordinal:04}"),
                None,
                Some(instance(8823, 600)),
            );
        }
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(
            closure_of(&ledger, active.generation).admissions.len(),
            RECOVERY_BATCH_LIMIT + 1
        );
    }

    #[test]
    fn malformed_legacy_suffix_after_committed_comparison_refuses_without_replacing_evidence() {
        for unequal in [false, true] {
            for damage in 0..6 {
                let directory = TempDir::new().unwrap();
                let (ledger, active) = open_generation(&directory);
                let rows: Vec<_> = ["a", "b"]
                    .into_iter()
                    .map(|id| {
                        let identity = admit(
                            directory.path(),
                            active.generation,
                            id,
                            None,
                            Some(instance(8840, 600)),
                        )
                        .unwrap();
                        serde_json::to_string(&SealedAdmission {
                            launch_id: id.to_owned(),
                            service: None,
                            identity,
                        })
                        .unwrap()
                    })
                    .collect();
                let first = if unequal {
                    rows[0].replace("501", "502")
                } else {
                    rows[0].clone()
                };
                let second = &rows[1];
                let bytes = match damage {
                    0 => format!("[{first}{second}]"),
                    1 => format!("[{first},,{second}]"),
                    2 => format!("[{first},{second},]"),
                    3 => format!("[{first},{second}"),
                    4 => format!("[{first},{second}] trailing"),
                    _ => format!("[{first},{{\"broken\":"),
                }
                .into_bytes();
                let path = ledger.sealed_ledger_path(active.generation);
                fs::write(&path, &bytes).unwrap();
                let (source, retirer) = fake(&[], &[], &[]);
                let lease = acquire_lease(&ledger);
                ASSEMBLY_SEGMENT_LIMIT.with(|n| n.set(1));
                LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(1));
                let first = ledger.reserve_generation_closing_abandoned(
                    instance(12, 4),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(10)),
                );
                ASSEMBLY_SEGMENT_LIMIT.with(|n| n.set(0));
                LEGACY_STOP_AFTER_SEGMENT.with(|n| n.set(0));
                assert!(first.is_err());
                let phase = ledger.closure_recovery_phase_dir(active.generation);
                let progress: serde_json::Value = serde_json::from_slice(
                    &fs::read(phase.join("legacy-comparison-progress.json")).unwrap(),
                )
                .unwrap();
                assert_eq!(progress["segments"].as_array().unwrap().len(), 1);
                let names = || {
                    let mut paths: Vec<_> = fs::read_dir(ledger.generation_path(active.generation))
                        .unwrap()
                        .map(|e| e.unwrap().file_name())
                        .collect();
                    paths.sort();
                    paths
                };
                let before = names();
                assert!(
                    ledger
                        .reserve_generation_closing_abandoned(
                            instance(13, 5),
                            [],
                            &lease,
                            &authority(&source, &retirer, Duration::from_secs(10))
                        )
                        .is_err(),
                    "unequal={unequal}, damage={damage}"
                );
                assert_eq!(fs::read(&path).unwrap(), bytes);
                assert_eq!(names(), before, "legacy was not set aside or replaced");
                assert_eq!(
                    ledger.active_generation().unwrap().unwrap().generation,
                    active.generation
                );
                assert!(!phase.join("publication.json").exists());
            }
        }
    }

    fn pause_after_publication(
        ledger: &ParentLossLedger,
        lease: &CoordinatorLease,
        source: &FakeSource,
        retirer: &FakeRetirer,
    ) {
        PUBLICATION_STOP_FAULT.with(|fault| fault.set(true));
        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            lease,
            &authority(source, retirer, Duration::from_secs(10)),
        );
        PUBLICATION_STOP_FAULT.with(|fault| fault.set(false));
        assert!(result.is_err());
    }

    #[test]
    fn prepared_publication_finishes_without_scan_or_historical_payloads() {
        for ledger_published in [false, true] {
            for damage_caches in [false, true] {
                let directory = TempDir::new().unwrap();
                let (ledger, active) = open_generation(&directory);
                admit(
                    directory.path(),
                    active.generation,
                    "prepared-resume",
                    None,
                    Some(instance(8824, 600)),
                );
                let (source, retirer) = fake(&[], &[], &[]);
                let lease = acquire_lease(&ledger);
                pause_after_publication(&ledger, &lease, &source, &retirer);
                let phase = ledger.closure_recovery_phase_dir(active.generation);
                let expected_record = fs::read(phase.join("record.prepared")).unwrap();
                let expected_ledger = fs::read(phase.join("ledger.prepared")).unwrap();
                if ledger_published {
                    fs::write(
                        ledger.sealed_ledger_path(active.generation),
                        &expected_ledger,
                    )
                    .unwrap();
                }
                let caches = [
                    ledger.closure_recovery_scan_complete_path(active.generation),
                    ledger.closure_recovery_batch_path(active.generation, 1),
                    ledger.closure_recovery_ids_path(active.generation, 1),
                ];
                for path in &caches {
                    if damage_caches {
                        fs::write(path, b"damaged historical cache").unwrap();
                    } else {
                        fs::remove_file(path).unwrap();
                    }
                }
                HISTORICAL_DISCOVERIES.with(|count| count.set(0));
                AUDIT_BATCH_READS.with(|count| count.set(0));
                ledger
                    .reserve_generation_closing_abandoned(
                        instance(12, 4),
                        [],
                        &lease,
                        &authority(&source, &retirer, Duration::from_secs(10)),
                    )
                    .unwrap();
                assert_eq!(HISTORICAL_DISCOVERIES.with(|count| count.get()), 0);
                assert_eq!(AUDIT_BATCH_READS.with(|count| count.get()), 0);
                assert_eq!(
                    fs::read(ledger.record_path(active.generation)).unwrap(),
                    expected_record
                );
                assert_eq!(
                    fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
                    expected_ledger
                );
                for path in &caches {
                    if damage_caches {
                        assert_eq!(fs::read(path).unwrap(), b"damaged historical cache");
                    } else {
                        assert!(!path.exists());
                    }
                }
            }
        }
    }

    #[test]
    fn committed_publication_refuses_damaged_prepared_or_conflicting_final_bytes() {
        for (ledger_published, damaged) in [
            (false, "ledger.prepared"),
            (false, "record.prepared"),
            (true, "record.prepared"),
            (false, "ledger.json"),
        ] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            admit(
                directory.path(),
                active.generation,
                "prepared-damage",
                None,
                Some(instance(8825, 600)),
            );
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            pause_after_publication(&ledger, &lease, &source, &retirer);
            let phase = ledger.closure_recovery_phase_dir(active.generation);
            if ledger_published {
                fs::copy(
                    phase.join("ledger.prepared"),
                    ledger.sealed_ledger_path(active.generation),
                )
                .unwrap();
            }
            let path = if damaged == "ledger.json" {
                ledger.sealed_ledger_path(active.generation)
            } else {
                phase.join(damaged)
            };
            fs::write(&path, b"[]").unwrap();
            assert!(
                ledger
                    .reserve_generation_closing_abandoned(
                        instance(12, 4),
                        [],
                        &lease,
                        &authority(&source, &retirer, Duration::from_secs(10)),
                    )
                    .is_err()
            );
            assert_eq!(fs::read(path).unwrap(), b"[]");
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
            let record: ParentLossGenerationRecord =
                serde_json::from_slice(&fs::read(ledger.record_path(active.generation)).unwrap())
                    .unwrap();
            assert!(record.closure.is_none());
        }
    }

    fn pause_after_assembly_segment(
        ledger: &ParentLossLedger,
        lease: &CoordinatorLease,
        source: &FakeSource,
        retirer: &FakeRetirer,
    ) {
        ASSEMBLY_SEGMENT_LIMIT.with(|limit| limit.set(1));
        ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(1));
        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            lease,
            &authority(source, retirer, Duration::from_secs(10)),
        );
        ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(0));
        ASSEMBLY_SEGMENT_LIMIT.with(|limit| limit.set(0));
        assert!(result.is_err());
    }

    #[test]
    fn committed_assembly_resumes_before_reading_or_decoding_its_completed_prefix() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        for id in ["b", "a"] {
            admit(
                directory.path(),
                active.generation,
                id,
                None,
                Some(instance(8826, 600)),
            );
        }
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        pause_after_assembly_segment(&ledger, &lease, &source, &retirer);
        ASSEMBLY_CONSUMED_ROWS.with(|count| count.set(0));
        ASSEMBLY_SPOOL_READ_BYTES.with(|count| count.set(0));
        ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(1));
        let second = ledger.reserve_generation_closing_abandoned(
            instance(12, 4),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(10)),
        );
        ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(0));
        assert!(second.is_err());
        assert_eq!(
            ASSEMBLY_CONSUMED_ROWS.with(|count| count.get()),
            1,
            "the first row must not be replayed"
        );
        assert_eq!(
            ASSEMBLY_SPOOL_READ_BYTES.with(|count| count.get()),
            0,
            "new progress precedes any completed-spool read/hash"
        );
        ledger
            .reserve_generation_closing_abandoned(
                instance(13, 5),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            )
            .unwrap();
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(
            closure
                .admissions
                .iter()
                .map(|row| row.launch_id.as_str())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        assert_eq!(
            sealed
                .iter()
                .map(|row| row["launch_id"].as_str().unwrap())
                .collect::<Vec<_>>(),
            ["a", "b"]
        );
    }

    #[test]
    fn paired_assembly_spools_recover_uncommitted_tails_at_both_write_boundaries() {
        for fault in [1, 2] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            for id in ["b", "a"] {
                admit(
                    directory.path(),
                    active.generation,
                    id,
                    None,
                    Some(instance(8827, 600)),
                );
            }
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            pause_after_assembly_segment(&ledger, &lease, &source, &retirer);
            let progress = ledger
                .closure_recovery_phase_dir(active.generation)
                .join("assembly-progress.json");
            let committed = fs::read(&progress).unwrap();
            ASSEMBLY_WRITE_FAULT.with(|value| value.set(fault));
            let interrupted = ledger.reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            );
            ASSEMBLY_WRITE_FAULT.with(|value| value.set(0));
            assert!(interrupted.is_err());
            assert_eq!(fs::read(&progress).unwrap(), committed);
            ledger
                .reserve_generation_closing_abandoned(
                    instance(13, 5),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(10)),
                )
                .unwrap();
            assert_eq!(
                closure_of(&ledger, active.generation)
                    .admissions
                    .iter()
                    .map(|row| row.launch_id.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"]
            );
            let sealed: Vec<serde_json::Value> = serde_json::from_slice(
                &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            )
            .unwrap();
            assert_eq!(sealed.len(), 2);
        }
    }

    #[test]
    fn damaged_merge_counts_positions_or_spans_cannot_publish_an_incomplete_prefix() {
        for damage in 0..6 {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            for id in ["b", "a"] {
                admit(
                    directory.path(),
                    active.generation,
                    id,
                    None,
                    Some(instance(8828, 600)),
                );
            }
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            pause_after_assembly_segment(&ledger, &lease, &source, &retirer);
            let path = ledger
                .closure_recovery_phase_dir(active.generation)
                .join("assembly-progress.json");
            let mut progress: serde_json::Value =
                serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
            match damage {
                0 => progress["positions"][0] = 2.into(),
                1 => progress["rows"] = 2.into(),
                2 => {
                    progress["spans"][0]["admission_end"] =
                        (progress["admission_end"].as_u64().unwrap() + 1).into()
                }
                3 => progress["schema"] = 9.into(),
                4 => progress["sealed_phase"] = "admitting".into(),
                _ => {
                    progress["positions"][0] = 2.into();
                    progress["rows"] = 2.into();
                    progress["spans"][0]["last_row"] = 2.into();
                }
            }
            write_json(&path, &progress, json_options()).unwrap();
            ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(1));
            let first = ledger.reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            );
            ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(0));
            assert!(first.is_err());
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
            ledger
                .reserve_generation_closing_abandoned(
                    instance(13, 5),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(10)),
                )
                .unwrap();
            assert_eq!(
                closure_of(&ledger, active.generation)
                    .admissions
                    .iter()
                    .map(|row| row.launch_id.as_str())
                    .collect::<Vec<_>>(),
                ["a", "b"]
            );
        }
    }

    #[test]
    fn altered_committed_spools_refuse_then_rebuild_and_runs_repair_from_their_committed_source() {
        for damage_spool in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            for id in ["b", "a"] {
                admit(
                    directory.path(),
                    active.generation,
                    id,
                    None,
                    Some(instance(8829, 600)),
                );
            }
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            pause_after_assembly_segment(&ledger, &lease, &source, &retirer);
            let phase = ledger.closure_recovery_phase_dir(active.generation);
            let path = phase.join(if damage_spool {
                "assembly-admissions.body"
            } else {
                "assembly-run-000001.ndjson"
            });
            let bytes = fs::read(&path).unwrap();
            let text = String::from_utf8(bytes).unwrap().replace("501", "502");
            fs::write(&path, text.as_bytes()).unwrap();
            let recovered = ledger.reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            );
            if damage_spool {
                assert!(recovered.is_err());
                assert_eq!(
                    ledger.active_generation().unwrap().unwrap().generation,
                    active.generation
                );
                ledger
                    .reserve_generation_closing_abandoned(
                        instance(13, 5),
                        [],
                        &lease,
                        &authority(&source, &retirer, Duration::from_secs(10)),
                    )
                    .unwrap();
            } else {
                recovered.unwrap();
            }
            assert!(
                closure_of(&ledger, active.generation)
                    .admissions
                    .iter()
                    .all(|row| row.uid == Some(501))
            );
        }
    }

    #[test]
    fn all_provisional_assembly_segments_resume_with_empty_sealed_spans() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        for id in ["b", "a"] {
            write_parent_loss_admission_intent(
                directory.path(),
                &AdmissionIntent::new(active.generation, id, None, None),
            )
            .unwrap();
        }
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        pause_after_assembly_segment(&ledger, &lease, &source, &retirer);
        let progress: serde_json::Value = serde_json::from_slice(
            &fs::read(
                ledger
                    .closure_recovery_phase_dir(active.generation)
                    .join("assembly-progress.json"),
            )
            .unwrap(),
        )
        .unwrap();
        assert_eq!(progress["sealed_end"], 0);
        assert_eq!(progress["spans"][0]["sealed_rows"], 0);
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(closure_of(&ledger, active.generation).admissions.len(), 2);
        assert_eq!(
            fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            b"[]\n"
        );
    }

    #[test]
    fn malformed_legacy_json_refuses_without_setting_aside_the_existing_ledger() {
        for bytes in [
            b"[".as_slice(),
            b"[] trailing",
            b"[{\"unequal\":true},",
            b"[{\"unequal\":true}] trailing",
        ] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            admit(
                directory.path(),
                active.generation,
                "legacy-malformed",
                None,
                Some(instance(8830, 600)),
            );
            let path = ledger.sealed_ledger_path(active.generation);
            fs::write(&path, bytes).unwrap();
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            assert!(
                ledger
                    .reserve_generation_closing_abandoned(
                        instance(11, 3),
                        [],
                        &lease,
                        &authority(&source, &retirer, Duration::from_secs(10))
                    )
                    .is_err()
            );
            assert_eq!(fs::read(path).unwrap(), bytes);
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
        }
    }

    #[test]
    fn legacy_staging_uses_the_exact_snapshot_that_passed_semantic_comparison() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let expected = admit(
            directory.path(),
            active.generation,
            "legacy-snapshot",
            None,
            Some(instance(8831, 600)),
        )
        .unwrap();
        let original = serde_json::to_vec_pretty(&vec![
            serde_json::json!({"launch_id":"legacy-snapshot","service":null,"identity":expected}),
        ])
        .unwrap();
        let path = ledger.sealed_ledger_path(active.generation);
        fs::write(&path, &original).unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        LEGACY_LEDGER_MUTATION_FAULT.with(|fault| fault.set(true));
        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(10)),
        );
        LEGACY_LEDGER_MUTATION_FAULT.with(|fault| fault.set(false));
        assert!(result.is_err());
        assert_eq!(fs::read(path).unwrap(), b"[]");
        assert_eq!(
            fs::read(
                ledger
                    .closure_recovery_phase_dir(active.generation)
                    .join("ledger.prepared")
            )
            .unwrap(),
            original
        );
        assert_eq!(
            ledger.active_generation().unwrap().unwrap().generation,
            active.generation
        );
    }

    #[test]
    fn changed_committed_ids_bytes_reset_partial_assembly_even_when_row_bytes_are_equal() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        for id in ["b", "a"] {
            admit(
                directory.path(),
                active.generation,
                id,
                None,
                Some(instance(8832, 600)),
            );
        }
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        pause_after_assembly_segment(&ledger, &lease, &source, &retirer);
        let phase = ledger.closure_recovery_phase_dir(active.generation);
        let progress_path = phase.join("assembly-progress.json");
        let before: serde_json::Value =
            serde_json::from_slice(&fs::read(&progress_path).unwrap()).unwrap();
        let ids_path = ledger.closure_recovery_ids_path(active.generation, 1);
        let ids: serde_json::Value = serde_json::from_slice(&fs::read(&ids_path).unwrap()).unwrap();
        fs::write(&ids_path, serde_json::to_vec_pretty(&ids).unwrap()).unwrap();
        ASSEMBLY_SEGMENT_LIMIT.with(|limit| limit.set(1));
        ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(1));
        let second = ledger.reserve_generation_closing_abandoned(
            instance(12, 4),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(10)),
        );
        ASSEMBLY_SEGMENT_LIMIT.with(|limit| limit.set(0));
        ASSEMBLY_STOP_AFTER_SEGMENT.with(|remaining| remaining.set(0));
        assert!(second.is_err());
        let after: serde_json::Value =
            serde_json::from_slice(&fs::read(progress_path).unwrap()).unwrap();
        assert_ne!(after["input_digest"], before["input_digest"]);
        assert_eq!(
            after["rows"], 1,
            "changed committed inputs restart derived assembly"
        );
        ledger
            .reserve_generation_closing_abandoned(
                instance(13, 5),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(closure_of(&ledger, active.generation).admissions.len(), 2);
    }

    #[test]
    fn acknowledgement_during_final_copies_or_finish_refuses_then_reopens_its_own_publication() {
        for boundary in 0..3 {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let id = "ack-during-publication";
            write_parent_loss_admission_intent(
                directory.path(),
                &AdmissionIntent::new(active.generation, id, None, None),
            )
            .unwrap();
            let root = instance(8833, 600);
            let expected = AdmissionIdentity {
                generation: active.generation,
                launch_id: id.to_owned(),
                instance: root,
                uid: 501,
                parent_launch_id: None,
            };
            let (source, retirer) = fake(&[(root, 501)], &[root.pid], &[]);
            let lease = acquire_lease(&ledger);
            if boundary == 2 {
                pause_after_publication(&ledger, &lease, &source, &retirer);
            }
            let fault = if boundary == 0 {
                &FINALIZATION_ACK_FAULT
            } else {
                &FINISH_ACK_FAULT
            };
            fault.with(|value| *value.borrow_mut() = Some(expected.clone()));
            let first = ledger.reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            );
            fault.with(|value| value.borrow_mut().take());
            assert!(
                first.is_err(),
                "late identity must veto this allocation, boundary={boundary}"
            );
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
            ledger
                .reserve_generation_closing_abandoned(
                    instance(13, 5),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(10)),
                )
                .unwrap();
            let closure = closure_of(&ledger, active.generation);
            assert_eq!(closure.admissions.len(), 1);
            assert_eq!(closure.admissions[0].instance, Some(root));
            assert_eq!(closure.admissions[0].uid, Some(501));
            assert_eq!(
                closure.admissions[0].finding,
                AdmissionFinding::Retired { escalated: false }
            );
        }
    }

    fn pause_after_audit(
        ledger: &ParentLossLedger,
        lease: &CoordinatorLease,
        source: &FakeSource,
        retirer: &FakeRetirer,
    ) {
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(1));
        let result = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            lease,
            &authority(source, retirer, Duration::from_secs(10)),
        );
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(0));
        assert!(result.is_err());
    }

    #[test]
    fn retained_scan_bypasses_discovery_and_survives_an_expired_publication_slice() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let expected = admit(
            directory.path(),
            active.generation,
            "scan-resume",
            None,
            Some(instance(8820, 600)),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        pause_after_audit(&ledger, &lease, &source, &retirer);
        let scan = ledger.closure_recovery_scan_complete_path(active.generation);
        let scan_bytes = fs::read(&scan).unwrap();
        let progress = ledger
            .closure_recovery_phase_dir(active.generation)
            .join("audit-progress.json");
        let progress_bytes = fs::read(&progress).unwrap();
        HISTORICAL_DISCOVERIES.with(|count| count.set(0));
        SCAN_PUBLICATIONS.with(|count| count.set(0));
        let result = ledger.reserve_generation_closing_abandoned(
            instance(12, 4),
            [],
            &lease,
            &authority(&source, &retirer, Duration::ZERO),
        );
        assert!(result.is_err());
        assert_eq!(HISTORICAL_DISCOVERIES.with(|count| count.get()), 0);
        assert_eq!(SCAN_PUBLICATIONS.with(|count| count.get()), 1);
        assert_eq!(
            fs::read(&scan).unwrap(),
            scan_bytes,
            "actual deadline keeps scheduling progress"
        );
        assert_eq!(fs::read(progress).unwrap(), progress_bytes);
        assert_eq!(
            ledger.active_generation().unwrap().unwrap().generation,
            active.generation
        );
        AUDIT_BATCH_READS.with(|count| count.set(0));
        ledger
            .reserve_generation_closing_abandoned(
                instance(13, 5),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(HISTORICAL_DISCOVERIES.with(|count| count.get()), 0);
        assert_eq!(SCAN_PUBLICATIONS.with(|count| count.get()), 2);
        assert_eq!(AUDIT_BATCH_READS.with(|count| count.get()), 0);
        assert_eq!(
            fs::read(scan).unwrap(),
            scan_bytes,
            "valid scan is not rewritten"
        );
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        assert_eq!(sealed.len(), 1);
        assert_eq!(
            serde_json::from_value::<AdmissionIdentity>(sealed[0]["identity"].clone()).unwrap(),
            expected
        );
    }

    #[test]
    fn damaged_scan_hints_are_preserved_and_rebuilt_without_early_allocation() {
        for damage in 0..4 {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let expected = admit(
                directory.path(),
                active.generation,
                "scan-damage",
                None,
                Some(instance(8821, 600)),
            )
            .unwrap();
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            pause_after_audit(&ledger, &lease, &source, &retirer);
            let scan = ledger.closure_recovery_scan_complete_path(active.generation);
            let mut hint: ScanCompleteFile =
                serde_json::from_slice(&fs::read(&scan).unwrap()).unwrap();
            match damage {
                0 => hint.ids_sequences = vec![999],
                1 => hint.ids_sequences.push(hint.ids_sequences[0]),
                2 => {
                    hint.sorted_launch_ids.pop();
                }
                _ => hint
                    .sorted_launch_ids
                    .push(hint.sorted_launch_ids[0].clone()),
            }
            write_json(&scan, &hint, json_options()).unwrap();
            let damaged_bytes = fs::read(&scan).unwrap();
            DISCOVERY_STOP_FAULT.with(|fault| fault.set(true));
            let first = ledger.reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            );
            DISCOVERY_STOP_FAULT.with(|fault| fault.set(false));
            assert!(first.is_err());
            assert!(
                !scan.exists(),
                "pre-deadline rejection must discard the scheduling hint"
            );
            assert!(
                fs::read_dir(scan.parent().unwrap())
                    .unwrap()
                    .flatten()
                    .any(|entry| fs::read(entry.path()).is_ok_and(|bytes| bytes == damaged_bytes))
            );
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
            let record: ParentLossGenerationRecord =
                serde_json::from_slice(&fs::read(ledger.record_path(active.generation)).unwrap())
                    .unwrap();
            assert!(record.closure.is_none());
            ledger
                .reserve_generation_closing_abandoned(
                    instance(13, 5),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(10)),
                )
                .unwrap();
            let sealed: Vec<serde_json::Value> = serde_json::from_slice(
                &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            )
            .unwrap();
            assert_eq!(sealed.len(), 1);
            assert_eq!(
                serde_json::from_value::<AdmissionIdentity>(sealed[0]["identity"].clone()).unwrap(),
                expected
            );
        }
    }

    #[test]
    fn priority_retirement_that_already_flushed_a_batch_reconstructs_the_scan() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "history-before-live",
            None,
            Some(instance(8822, 600)),
        )
        .unwrap();
        let root = instance(8823, 600);
        let launch_id = "priority-live";
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(active.generation, launch_id, None, None),
        )
        .unwrap();
        let (source, retirer) = fake(&[(root, 501)], &[root.pid], &[]);
        let lease = acquire_lease(&ledger);
        pause_after_audit(&ledger, &lease, &source, &retirer);
        let expected = AdmissionIdentity {
            generation: active.generation,
            launch_id: launch_id.to_owned(),
            instance: root,
            uid: 501,
            parent_launch_id: None,
        };
        acknowledge_parent_loss_admission(directory.path(), expected.clone()).unwrap();
        let derived = read_derived_admission_drops(&ledger, active.generation, launch_id).unwrap();
        let live = LiveRecoveryFile {
            sources: Some(derived.sources),
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_owned(),
            service: None,
            root,
            uid: 501,
            parent_launch_id: None,
            descendants: Vec::new(),
            binding_digest: String::new(),
            escalate_at_unix_ms: 0,
            reap_deadline_unix_ms: None,
            term_requested: false,
            escalated: false,
        };
        persist_live(&ledger, active.generation, &live).unwrap();
        // Promotion has transferred the obligation to the authoritative census.
        // Leave no provisional mismatch that could independently block the shortcut.
        fs::remove_file(ledger.closure_recovery_provisional_path(active.generation, launch_id))
            .unwrap();
        HISTORICAL_DISCOVERIES.with(|count| count.set(0));
        SCAN_PUBLICATIONS.with(|count| count.set(0));
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(10)),
            )
            .unwrap();
        assert_eq!(HISTORICAL_DISCOVERIES.with(|count| count.get()), 1);
        assert_eq!(
            SCAN_PUBLICATIONS.with(|count| count.get()),
            0,
            "a drained buffer does not make a newly flushed completion part of an old scan"
        );
        let closure = closure_of(&ledger, active.generation);
        let retired = closure
            .admissions
            .iter()
            .find(|row| row.launch_id == launch_id)
            .unwrap();
        assert_eq!(retired.instance, Some(root));
        assert_eq!(
            retired.finding,
            AdmissionFinding::Retired { escalated: false }
        );
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        assert_eq!(sealed.len(), 2);
        assert!(sealed.iter().any(|row| {
            serde_json::from_value::<AdmissionIdentity>(row["identity"].clone())
                .is_ok_and(|actual| actual == expected)
        }));
    }

    #[test]
    fn committed_audit_progress_skips_prior_source_validation_but_includes_late_admissions() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let mut expected = Vec::new();
        for (id, pid) in [("audit-one", 8810), ("audit-two", 8811)] {
            expected.push(
                admit(
                    directory.path(),
                    active.generation,
                    id,
                    None,
                    Some(instance(pid, 600)),
                )
                .unwrap(),
            );
        }
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        AUDIT_VALIDATED_ROWS.with(|count| count.set(0));
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(1));
        let first = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(2)),
        );
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(0));
        assert!(first.is_err());
        assert_eq!(AUDIT_VALIDATED_ROWS.with(|count| count.get()), 2);
        assert!(!ledger.sealed_ledger_path(active.generation).exists());

        expected.push(
            admit(
                directory.path(),
                active.generation,
                "audit-late",
                None,
                Some(instance(8812, 600)),
            )
            .unwrap(),
        );
        AUDIT_VALIDATED_ROWS.with(|count| count.set(0));
        AUDIT_BATCH_READS.with(|count| count.set(0));
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(
            AUDIT_VALIDATED_ROWS.with(|count| count.get()),
            1,
            "a committed source audit must leave the retry budget for unfinished work"
        );
        assert_eq!(
            AUDIT_BATCH_READS.with(|count| count.get()),
            1,
            "a retained committed chunk must not reread its completed raw batch"
        );
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        assert_eq!(sealed.len(), expected.len());
        for identity in expected {
            assert!(sealed.iter().any(|row| {
                serde_json::from_value::<AdmissionIdentity>(row["identity"].clone())
                    .is_ok_and(|actual| actual == identity)
            }));
        }
    }

    #[test]
    fn malformed_deferred_payload_is_preserved_and_rebuilt_from_originals() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let expected = admit(
            directory.path(),
            active.generation,
            "damaged-payload",
            None,
            Some(instance(8813, 600)),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(1));
        let first = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(2)),
        );
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(0));
        assert!(first.is_err());
        // Without the independent commit, this raw payload must be audited again.
        fs::remove_file(
            ledger
                .closure_recovery_phase_dir(active.generation)
                .join("audit-progress.json"),
        )
        .unwrap();
        let batch = ledger.closure_recovery_batch_path(active.generation, 1);
        fs::write(&batch, b"{damaged payload").unwrap();
        let recovered = ledger.reserve_generation_closing_abandoned(
            instance(12, 4),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(2)),
        );
        assert!(!batch.exists());
        assert!(fs::read_dir(batch.parent().unwrap()).unwrap().flatten().any(|entry|
            fs::read(entry.path()).is_ok_and(|bytes| bytes == b"{damaged payload")));
        if recovered.is_err() {
            ledger
                .reserve_generation_closing_abandoned(
                    instance(13, 5),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2)),
                )
                .unwrap();
        }
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        assert_eq!(sealed.len(), 1);
        assert_eq!(
            serde_json::from_value::<AdmissionIdentity>(sealed[0]["identity"].clone()).unwrap(),
            expected
        );
    }

    #[test]
    fn committed_output_survives_damage_to_its_historical_raw_batch() {
        for malformed in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let expected = admit(
                directory.path(),
                active.generation,
                "committed-payload",
                None,
                Some(instance(8814, 600)),
            )
            .unwrap();
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(1));
            let first = ledger.reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            );
            AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(0));
            assert!(first.is_err());
            let batch = ledger.closure_recovery_batch_path(active.generation, 1);
            let bytes = if malformed {
                b"{damaged payload".to_vec()
            } else {
                let mut payload: serde_json::Value =
                    serde_json::from_slice(&fs::read(&batch).unwrap()).unwrap();
                payload["entries"][0]["uid"] = 502.into();
                serde_json::to_vec(&payload).unwrap()
            };
            fs::write(&batch, &bytes).unwrap();
            AUDIT_BATCH_READS.with(|count| count.set(0));
            ledger
                .reserve_generation_closing_abandoned(
                    instance(12, 4),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2)),
                )
                .unwrap();
            assert_eq!(AUDIT_BATCH_READS.with(|count| count.get()), 0);
            assert_eq!(
                fs::read(batch).unwrap(),
                bytes,
                "preserve damaged historical evidence"
            );
            let closure = closure_of(&ledger, active.generation);
            assert_eq!(closure.admissions[0].uid, Some(expected.uid));
            let sealed: Vec<serde_json::Value> = serde_json::from_slice(
                &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            )
            .unwrap();
            assert_eq!(sealed.len(), 1);
            assert_eq!(
                serde_json::from_value::<AdmissionIdentity>(sealed[0]["identity"].clone()).unwrap(),
                expected,
            );
        }
    }

    #[test]
    fn changed_committed_ids_bytes_force_a_fresh_source_audit() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let expected = admit(
            directory.path(),
            active.generation,
            "committed-ids",
            None,
            Some(instance(8815, 600)),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(1));
        let first = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(2)),
        );
        AUDIT_STOP_AFTER_BATCH.with(|remaining| remaining.set(0));
        assert!(first.is_err());
        let ids = ledger.closure_recovery_ids_path(active.generation, 1);
        let before = fs::read(&ids).unwrap();
        let value: serde_json::Value = serde_json::from_slice(&before).unwrap();
        let changed = serde_json::to_vec(&value).unwrap();
        assert_ne!(changed, before, "same IDs, different serialized bytes");
        fs::write(ids, changed).unwrap();
        AUDIT_BATCH_READS.with(|count| count.set(0));
        AUDIT_VALIDATED_ROWS.with(|count| count.set(0));
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(AUDIT_BATCH_READS.with(|count| count.get()), 1);
        assert_eq!(AUDIT_VALIDATED_ROWS.with(|count| count.get()), 1);
        let sealed: Vec<serde_json::Value> = serde_json::from_slice(
            &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
        )
        .unwrap();
        assert_eq!(sealed.len(), 1);
        assert_eq!(
            serde_json::from_value::<AdmissionIdentity>(sealed[0]["identity"].clone()).unwrap(),
            expected,
        );
    }

    #[test]
    fn a_rehashed_prepared_chunk_cannot_override_its_unchanged_authorized_batch() {
        for damaged_progress in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let expected = admit(
                directory.path(),
                active.generation,
                "prepared-uid",
                None,
                Some(instance(8806, 600)),
            )
            .unwrap();
            let ack_path = ledger
                .generation_path(active.generation)
                .join("admissions/prepared-uid/acknowledgement.json");
            let original_ack = fs::read(&ack_path).unwrap();
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            PUBLICATION_STOP_FAULT.with(|fault| fault.set(true));
            let first = ledger.reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            );
            PUBLICATION_STOP_FAULT.with(|fault| fault.set(false));
            assert!(first.is_err());
            let phase = ledger.closure_recovery_phase_dir(active.generation);
            fs::remove_file(phase.join("publication.json")).unwrap();
            if damaged_progress {
                fs::write(phase.join("audit-progress.json"), b"broken audit progress").unwrap();
            }
            let chunk_path = phase.join("audited-000001.json");
            let mut chunk: serde_json::Value =
                serde_json::from_slice(&fs::read(&chunk_path).unwrap()).unwrap();
            let batch_before =
                fs::read(ledger.closure_recovery_batch_path(active.generation, 1)).unwrap();
            let mut sealed: serde_json::Value =
                serde_json::from_str(chunk["rows"][0]["sealed"].as_str().unwrap()).unwrap();
            sealed["identity"]["uid"] = 502.into();
            chunk["rows"][0]["sealed"] = serde_json::to_string(&sealed).unwrap().into();
            let bytes = serde_json::to_vec(&chunk).unwrap();
            fs::write(&chunk_path, &bytes).unwrap();
            fs::write(phase.join("audited-000001.sha256"), digest_bytes(&bytes)).unwrap();
            ledger
                .reserve_generation_closing_abandoned(
                    instance(12, 4),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2)),
                )
                .unwrap();
            let sealed: serde_json::Value = serde_json::from_slice(
                &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            )
            .unwrap();
            let actual: AdmissionIdentity =
                serde_json::from_value(sealed[0]["identity"].clone()).unwrap();
            assert_eq!(fs::read(&ack_path).unwrap(), original_ack);
            assert_eq!(
                fs::read(ledger.closure_recovery_batch_path(active.generation, 1)).unwrap(),
                batch_before
            );
            assert_eq!(
                actual, expected,
                "prepared cache must agree with unchanged source-authorized identity"
            );
        }
    }

    #[test]
    fn a_late_ack_after_record_publication_is_reaudited_even_after_record_set_aside() {
        for interrupted_after_aside in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let launch_id = "late-after-record";
            admit(directory.path(), active.generation, launch_id, None, None);
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            ledger
                .reserve_generation_closing_abandoned(
                    instance(11, 3),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2)),
                )
                .unwrap();
            let published_record = fs::read(ledger.record_path(active.generation)).unwrap();
            let mut sealed_pointer = active.clone();
            sealed_pointer.phase = ParentLossPhase::Sealed;
            write_json(ledger.active_path(), &sealed_pointer, json_options()).unwrap();
            let expected = admit(
                directory.path(),
                active.generation,
                launch_id,
                None,
                Some(instance(8805, 500)),
            )
            .unwrap();
            if interrupted_after_aside {
                let aside =
                    publication::supersede_overtaken_publication(&ledger, active.generation)
                        .unwrap()
                        .expect("exact overtaken publication is preserved");
                assert_eq!(fs::read(aside).unwrap(), published_record);
                assert!(!ledger.record_path(active.generation).exists());
            }
            ledger
                .reserve_generation_closing_abandoned(
                    instance(12, 4),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2)),
                )
                .expect("late acknowledgement must not permanently wedge a sealed predecessor");
            let closure = closure_of(&ledger, active.generation);
            assert_eq!(closure.admissions[0].instance, Some(expected.instance));
            let sealed: Vec<serde_json::Value> = serde_json::from_slice(
                &fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            )
            .unwrap();
            assert_eq!(sealed.len(), 1);
            let actual: AdmissionIdentity =
                serde_json::from_value(sealed[0]["identity"].clone()).unwrap();
            assert_eq!(actual, expected);
            assert!(
                fs::read_dir(ledger.generation_path(active.generation))
                    .unwrap()
                    .filter_map(Result::ok)
                    .any(|entry| {
                        entry
                            .file_name()
                            .to_string_lossy()
                            .starts_with("record.wedged-")
                            && fs::read(entry.path()).is_ok_and(|b| b == published_record)
                    })
            );
        }
    }

    #[test]
    fn post_publication_retry_reuses_exact_audited_artifacts_and_refuses_a_changed_record() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "published",
            None,
            Some(instance(8803, 300)),
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        // Simulate death after record publication but before successor-pointer publication.
        let mut sealed_pointer = active.clone();
        sealed_pointer.phase = ParentLossPhase::Sealed;
        write_json(ledger.active_path(), &sealed_pointer, json_options()).unwrap();
        // Once both audited artifacts are published, scheduling caches are expendable.
        fs::remove_file(ledger.closure_recovery_batch_path(active.generation, 1)).unwrap();
        fs::remove_file(ledger.closure_recovery_ids_path(active.generation, 1)).unwrap();
        let record_bytes = fs::read(ledger.record_path(active.generation)).unwrap();
        let ledger_bytes = fs::read(ledger.sealed_ledger_path(active.generation)).unwrap();
        ledger
            .reserve_generation_closing_abandoned(
                instance(12, 4),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .unwrap();
        assert_eq!(
            fs::read(ledger.record_path(active.generation)).unwrap(),
            record_bytes
        );
        assert_eq!(
            fs::read(ledger.sealed_ledger_path(active.generation)).unwrap(),
            ledger_bytes
        );
        write_json(ledger.active_path(), &sealed_pointer, json_options()).unwrap();
        let mut record: ParentLossGenerationRecord = serde_json::from_slice(&record_bytes).unwrap();
        record.closure.as_mut().unwrap().admissions[0].uid = Some(502);
        write_json(
            ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .unwrap();
        assert!(
            ledger
                .reserve_generation_closing_abandoned(
                    instance(13, 5),
                    [],
                    &lease,
                    &authority(&source, &retirer, Duration::from_secs(2))
                )
                .is_err()
        );
        assert_eq!(
            ledger.active_generation().unwrap().unwrap().generation,
            active.generation
        );
    }

    #[test]
    fn the_first_term_gets_its_full_grace_even_if_the_census_was_captured_earlier() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(8813, 400);
        let launch_id = "unstarted-retirement";
        admit(
            directory.path(),
            active.generation,
            launch_id,
            None,
            Some(root),
        );
        let derived = read_derived_admission_drops(&ledger, active.generation, launch_id).unwrap();
        let live = LiveRecoveryFile {
            sources: Some(derived.sources),
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_owned(),
            service: None,
            root,
            uid: 501,
            parent_launch_id: None,
            descendants: Vec::new(),
            binding_digest: compute_live_binding_digest(root, 501, &[]).unwrap(),
            escalate_at_unix_ms: unix_millis() - 1,
            reap_deadline_unix_ms: None,
            term_requested: false,
            escalated: false,
        };
        persist_live(&ledger, active.generation, &live).unwrap();
        let (source, retirer) = fake(&[(root, 501)], &[], &[root.pid]);
        let lease = acquire_lease(&ledger);
        let before = unix_millis();
        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_millis(50)),
            )
            .unwrap_err();
        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::CoordinatorNotLive)
        ));
        assert_eq!(*retirer.signals.lock().unwrap(), vec![(root.pid, "term")]);
        let saved = read_retirement_census(&ledger, active.generation).unwrap();
        assert!(saved.obligations[launch_id].escalate_at_unix_ms >= before + 12_500);
        assert!(!saved.obligations[launch_id].escalated);
    }

    #[test]
    fn stored_timing_literals_12500_and_500_clamp_downward_and_past_reap_refuses_live() {
        // Retirement policy is fixed independently of recovery's restart slice.
        assert_eq!(LIVE_ESCALATION_GRACE_MS, 12_500);
        assert_eq!(LIVE_REAP_GRACE_MS, 500);
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "timing-test";
        let live_inst = instance(9001, 100);
        admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(live_inst),
        );

        // Plant far-future escalate_at
        let now_ms = unix_millis();

        let live_file = LiveRecoveryFile {
            sources: None,
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            root: live_inst,
            uid: 501,
            parent_launch_id: None,
            descendants: Vec::new(),
            escalate_at_unix_ms: now_ms + 999_999,
            reap_deadline_unix_ms: None,
            term_requested: true,
            escalated: false,
            binding_digest: compute_live_binding_digest(live_inst, 501, &[]).unwrap(),
        };
        let obligations = HashMap::from([(launch_id.to_string(), live_file.clone())]);
        let ob_file = CensusObligationFile {
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            obligations: obligations.clone(),
            binding_digest: compute_census_obligation_digest(&obligations).unwrap(),
        };
        write_json(
            ledger.census_obligation_path(active.generation),
            &ob_file,
            json_options(),
        )
        .unwrap();

        let live_path = ledger.closure_recovery_live_path(active.generation, launch_id);
        fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        write_json(&live_path, &live_file, json_options()).unwrap();

        let (source, retirer) = fake(&[(live_inst, 501)], &[], &[live_inst.pid]);
        let lease = acquire_lease(&ledger);

        // Attempt returns without waiting 999s and clamps down:
        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_millis(50)),
            )
            .expect_err("attempt deadline hit");
        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(BootstrapRecoveryReason::CoordinatorNotLive)
        ));

        let persisted: LiveRecoveryFile =
            serde_json::from_slice(&fs::read(&live_path).unwrap()).unwrap();
        assert!(
            persisted.escalate_at_unix_ms <= now_ms + 12_500 + 500,
            "escalate_at clamped down to <= now + 12_500"
        );

        // Plant past reap deadline with process still SameLive:
        let past_reap_file = LiveRecoveryFile {
            sources: None,
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            root: live_inst,
            uid: 501,
            parent_launch_id: None,
            descendants: Vec::new(),
            escalate_at_unix_ms: now_ms.saturating_sub(1000),
            reap_deadline_unix_ms: Some(now_ms.saturating_sub(100)),
            term_requested: true,
            escalated: false,
            binding_digest: compute_live_binding_digest(live_inst, 501, &[]).unwrap(),
        };
        persist_live(&ledger, active.generation, &past_reap_file).unwrap();

        let error2 = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err(
                "past reap deadline with process still live returns AbandonedAdmissionLive",
            );
        assert!(matches!(
            error2,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionLive
            )
        ));
    }

    #[cfg(all(test, feature = "full-tests"))]
    struct ChildGuard(std::process::Child);

    #[cfg(all(test, feature = "full-tests"))]
    impl Drop for ChildGuard {
        fn drop(&mut self) {
            let _ = self.0.kill();
            let _ = self.0.wait();
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn spawn_test_child(ignore_sigterm: bool) -> (ChildGuard, ProcessInstance) {
        let fixture = option_env!("CARGO_BIN_EXE_solstone-system-test-child")
            .map(std::path::PathBuf::from)
            .or_else(|| {
                std::env::current_exe().ok().and_then(|current| {
                    let dir = current.parent()?;
                    let candidate = dir.join("solstone-system-test-child");
                    if candidate.is_file() {
                        Some(candidate)
                    } else {
                        let parent = dir.parent()?;
                        let candidate = parent.join("solstone-system-test-child");
                        if candidate.is_file() {
                            Some(candidate)
                        } else {
                            None
                        }
                    }
                })
            })
            .unwrap_or_else(|| std::path::PathBuf::from("solstone-system-test-child"));
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos();
        let ready = std::env::temp_dir().join(format!(
            "solstone-parent-loss-stubborn-{}-{stamp}",
            std::process::id()
        ));
        let mut command = std::process::Command::new(fixture);
        if ignore_sigterm {
            command.args(["block-term-sleep", ready.to_str().expect("utf8 path")]);
        } else {
            command.arg("sleep");
        }
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child");
        let guard = ChildGuard(child);
        if ignore_sigterm {
            for _ in 0..500 {
                if ready.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert!(ready.exists(), "stubborn child installed SIGTERM mask");
            let _ = fs::remove_file(&ready);
        }
        let instance = match SystemProcessInstanceSource.inspect(guard.0.id()) {
            InspectResult::Present { instance, .. } => instance,
            other => panic!("own child must be inspectable: {other:?}"),
        };
        (guard, instance)
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn real_block_term_sleep_child_with_handshake_respects_grace_and_retry_does_not_extend() {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let (_guard, inst) = spawn_test_child(true);

        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(
                active.generation,
                "stubborn-child",
                Some(HostedServiceKind::Sense),
                None,
            ),
        )
        .unwrap();
        let identity = AdmissionIdentity {
            generation: active.generation,
            launch_id: "stubborn-child".into(),
            instance: inst,
            uid: nix::unistd::getuid().as_raw(),
            parent_launch_id: None,
        };
        acknowledge_parent_loss_admission(directory.path(), identity.clone()).unwrap();
        write_parent_loss_admission_result(
            directory.path(),
            active.generation,
            "stubborn-child",
            &AdmissionResult {
                schema: 1,
                identity: Some(identity),
                state: AdmissionResultState::Admitted,
            },
        )
        .unwrap();

        let source = SystemProcessInstanceSource;
        let retirer = SystemAdmissionRetirer;
        let lease = acquire_lease(&ledger);

        let start = Instant::now();
        let outcome = ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            &lease,
            &authority(&source, &retirer, Duration::from_secs(30)),
        );
        let elapsed = start.elapsed();
        assert!(
            outcome.is_ok(),
            "eventual SIGKILL reaps stubborn child: {outcome:?}"
        );
        assert!(
            elapsed >= Duration::from_millis(12_000),
            "must wait out 12.5s grace before KILL"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn retained_census_descendant_obligation_after_root_exit_and_rebound_descendant_not_signalled()
    {
        let directory = TempDir::new().expect("temporary root");
        let (ledger, active) = open_generation(&directory);
        let launch_id = "census-obligation";
        let root_inst = instance(9901, 100);
        let desc_inst = instance(9902, 200);

        admit(
            directory.path(),
            active.generation,
            launch_id,
            Some(HostedServiceKind::Sense),
            Some(root_inst),
        );

        let live_file = LiveRecoveryFile {
            sources: None,
            schema: PARENT_LOSS_RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation: active.generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_string(),
            service: Some(HostedServiceKind::Sense),
            root: root_inst,
            uid: 501,
            parent_launch_id: None,
            descendants: vec![LiveDescendantIdentity {
                instance: desc_inst,
                uid: 501,
                ppid: root_inst.pid,
                pgid: root_inst.pid,
                escalate_at_unix_ms: 1,
                reap_deadline_unix_ms: Some(1),
                term_requested: true,
                escalated: true,
            }],
            escalate_at_unix_ms: 0,
            reap_deadline_unix_ms: None,
            term_requested: true,
            escalated: false,
            binding_digest: compute_live_binding_digest(
                root_inst,
                501,
                &[LiveDescendantIdentity {
                    instance: desc_inst,
                    uid: 501,
                    ppid: root_inst.pid,
                    pgid: root_inst.pid,
                    escalate_at_unix_ms: 1,
                    reap_deadline_unix_ms: Some(1),
                    term_requested: true,
                    escalated: true,
                }],
            )
            .unwrap(),
        };
        let live_path = ledger.closure_recovery_live_path(active.generation, launch_id);
        fs::create_dir_all(live_path.parent().unwrap()).unwrap();
        persist_live(&ledger, active.generation, &live_file).unwrap();

        // Root is exited, descendant is still live:
        let (source, retirer) = fake(&[(desc_inst, 501)], &[], &[desc_inst.pid]);
        let lease = acquire_lease(&ledger);

        let error = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect_err("live descendant must prevent completion");
        assert!(matches!(
            error,
            ParentLossLedgerError::RecoveryRequired(
                BootstrapRecoveryReason::AbandonedAdmissionLive
            )
        ));
        assert!(
            live_path.exists(),
            "live file retained while descendant live"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn semantically_different_existing_ledger_set_aside_and_rebuilt_while_conflicting_record_refuses()
     {
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

        // Plant different ledger:
        let diff_ledger = vec![SealedAdmission {
            launch_id: "different-id".to_string(),
            service: Some(HostedServiceKind::Sense),
            identity: AdmissionIdentity {
                generation: active.generation,
                launch_id: "different-id".to_string(),
                instance: instance(9999, 99),
                uid: 501,
                parent_launch_id: None,
            },
        }];
        let ledger_path = ledger.sealed_ledger_path(active.generation);
        fs::create_dir_all(ledger_path.parent().unwrap()).unwrap();
        write_json(&ledger_path, &diff_ledger, json_options()).unwrap();

        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        // Case 1: No record digest commits it -> set aside and rebuilt:
        let successor = ledger
            .reserve_generation_closing_abandoned(
                instance(11, 3),
                [],
                &lease,
                &authority(&source, &retirer, Duration::from_secs(2)),
            )
            .expect("rebuilds different ledger");
        assert_eq!(successor.generation, active.generation + 1);

        let closure = closure_of(&ledger, active.generation);
        assert!(
            closure
                .set_aside
                .iter()
                .any(|s| s.starts_with("ledger.wedged-") && s.ends_with(".json")),
            "conflicting ledger was set aside"
        );
    }
}
