// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! What a parent-loss generation still owes, and how a start settles it.
//!
//! `generations/<g>/admissions/` holds one entry per launch the run has not
//! discharged. An entry leaves it only by a rename into `discharged/`, under
//! the admission lock, once the process it names is proven gone: reaped by its
//! launcher, observed exited by exact identity, settled by the process table,
//! or ended by a changed boot session. So the work here is proportional to
//! what was outstanding, never to how long the run lasted.
//!
//! A start that finds a generation whose supervisor and coordinator both died
//! settles every entry the same way, retires what is still live by exact
//! signal, and closes the generation only when a fresh listing is empty and no
//! retirement obligation remains.
//!
//! Liveness of the recorded authorities is proven by lock and only vetoed by
//! identity: the caller holds the coordinator lease and its parent holds the
//! supervisor lock, and a `SameLive` observation of either still refuses.
//! ⛔ "Cannot observe" never settles anything.
//!
//! The only state that crosses starts besides the entries themselves is the
//! descendant census (`census-obligation.json`, `census-started.json`), kept in
//! the released binary's format so a rolled-back binary still honours it.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use solstone_core_journal_io::durability::{
    ArtifactId, DurableRead, read_json_durable, read_json_durable_validated,
};
use solstone_core_journal_io::{LockOptions, hold_lock, write_json};

use super::HostedServiceKind;
use super::parent_loss_admission::{
    AdmissionAcknowledgement, AdmissionIdentity, AdmissionIntent, AdmissionResult,
    AdmissionResultState, AdmissionSpawnIdentity, admission_directory, validate_launch_id,
};
use super::parent_loss_coordinator::{SealedAdmission, clear_supervisor_heartbeat};
use super::parent_loss_ledger::{
    ActiveGeneration, BootstrapRecoveryReason, PARENT_LOSS_LEDGER_SCHEMA_V1, ParentLossGeneration,
    ParentLossGenerationRecord, ParentLossLedger, ParentLossLedgerError, ParentLossPhase,
    ParentLossTerminalDisposition, ParentLossUnresolvedReason, canonical_json, digest_bytes,
    json_options,
};
#[cfg(any(target_os = "linux", target_os = "macos"))]
use crate::process::exact_descendant_tree;
use crate::process::{
    InstanceVerdict, ProcessInstance, ProcessInstanceSource, ProcessOwner, SignalKind,
    SystemProcessInstanceSource, TableBirth, TerminationError, parse_linux_boot_id, process_owner,
    process_table_birth, signal_exact_instance,
};

pub const PARENT_LOSS_CLOSURE_SCHEMA_V1: u32 = 1;
const RECOVERY_SCHEMA_V1: u32 = 1;
const BOOT_SCHEMA_V1: u32 = 1;
/// TERM to KILL, then KILL to giving up. Stored per identity as wall-clock
/// deadlines, so a later start resumes them instead of granting a fresh grace.
const TERM_GRACE_MS: u64 = 12_500;
const REAP_GRACE_MS: u64 = 500;
const POLL: Duration = Duration::from_millis(25);
const SETTLE_WINDOW: Duration = Duration::from_millis(500);
/// A hosted launch holds the admission lock through its child's
/// acknowledgement (up to 5 s); a seal waits a little longer than that.
pub(crate) const ADMISSION_SEAL_LOCK_TIMEOUT: Duration = Duration::from_secs(6);
const DISCHARGE_LOCK_TIMEOUT: Duration = Duration::from_secs(2);
/// Entries read ahead of observation, and by how many threads. Reading drops
/// is most of what settling a large legacy set costs.
const READ_WINDOW: usize = 1024;
const READ_THREADS: usize = 8;
/// The record keeps rows only for what is worth a person's eye, and at most
/// this many; the summary counts everything.
const NOTEWORTHY_ROW_LIMIT: usize = 1024;
/// Each pass settles a whole listing; a further pass only runs for entries
/// that appeared after it.
const MAX_PASSES: usize = 8;

// ---- the closure record (read by the released binary; format unchanged) ----

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
    /// An intent with no identity the closer could read: the child never
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

/// Counts of what a closure settled, by finding.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosureSummary {
    #[serde(default)]
    pub exited: u64,
    #[serde(default)]
    pub retired: u64,
    #[serde(default)]
    pub escalated: u64,
    #[serde(default)]
    pub never_acknowledged: u64,
    #[serde(default)]
    pub spawn_failed: u64,
    #[serde(default)]
    pub rejected_and_reaped: u64,
    #[serde(default)]
    pub pid_reused_by_another_user: u64,
    #[serde(default)]
    pub unidentified: u64,
    /// The computer restarted since the run, so the whole generation was
    /// settled at once and its entries were never read.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub boot_changed: bool,
}

impl ClosureSummary {
    pub fn total(&self) -> u64 {
        self.exited
            + self.retired
            + self.never_acknowledged
            + self.spawn_failed
            + self.rejected_and_reaped
            + self.pid_reused_by_another_user
            + self.unidentified
    }

    fn count(&mut self, finding: AdmissionFinding) {
        match finding {
            AdmissionFinding::Exited => self.exited += 1,
            AdmissionFinding::Retired { escalated } => {
                self.retired += 1;
                self.escalated += u64::from(escalated);
            }
            AdmissionFinding::NeverAcknowledged => self.never_acknowledged += 1,
            AdmissionFinding::SpawnFailed => self.spawn_failed += 1,
            AdmissionFinding::RejectedAndReaped => self.rejected_and_reaped += 1,
            AdmissionFinding::PidReusedByAnotherUser => self.pid_reused_by_another_user += 1,
            AdmissionFinding::Unidentified => self.unidentified += 1,
        }
    }
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
    /// Bookkeeping files set aside beside the record.
    pub set_aside: Vec<String>,
    /// Anomalies worth a human's eye, in plain words.
    pub notes: Vec<String>,
    /// Only the noteworthy rows: retired, or discharged without an identity.
    /// Records written before `summary` existed hold every row.
    pub admissions: Vec<AdmissionClosure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<ClosureSummary>,
}

// ---- the instruments ----

/// Sends exact signals during retirement and answers who owns a pid.
/// Injected so every refuse path is testable without a live process to kill.
pub trait AdmissionRetirer: Send + Sync {
    fn signal(&self, instance: ProcessInstance, signal: SignalKind)
    -> Result<(), TerminationError>;
    fn owner(&self, pid: u32) -> ProcessOwner;
    /// The birth of whatever holds `pid`, from the same table as `owner`.
    /// Without an instrument it settles nothing.
    fn table_birth(&self, pid: u32) -> TableBirth {
        let _ = pid;
        TableBirth::Unknown
    }
    /// The kernel's boot session id, or `None` when it cannot be read.
    fn boot_id(&self) -> Option<String> {
        current_boot_id()
    }
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

    fn table_birth(&self, pid: u32) -> TableBirth {
        process_table_birth(pid)
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ClosureOutcome {
    /// The closure is written into the generation's record.
    Closed,
    Refused(BootstrapRecoveryReason),
}

// ---- the descendant census (released format, byte-compatible) ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum RecoveryArtifactKind {
    Live,
}

/// The raw drop texts an obligation's root came from. The released binary
/// re-derives the root from these before it acts on an obligation.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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

/// One retirement obligation: a root and every descendant snapshotted before
/// it was signalled, with their TERM/KILL schedule.
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
pub(crate) struct CensusObligationFile {
    pub(crate) schema: u32,
    pub(crate) generation: ParentLossGeneration,
    pub(crate) sealed_phase: ParentLossPhase,
    pub(crate) obligations: HashMap<String, LiveRecoveryFile>,
    pub(crate) binding_digest: String,
}

pub(crate) type Obligations = HashMap<String, LiveRecoveryFile>;

pub(crate) fn compute_census_obligation_digest(
    obligations: &Obligations,
) -> Result<String, ParentLossLedgerError> {
    let mut entries: Vec<(&String, &LiveRecoveryFile)> = obligations.iter().collect();
    entries.sort_by_key(|(k, _)| *k);
    let bytes = canonical_json(&entries)?;
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
    let bytes = canonical_json(&CanonicalLiveBinding {
        root,
        uid,
        descendants,
    })?;
    Ok(digest_bytes(&bytes))
}

pub(crate) fn unix_millis() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_millis().min(u64::MAX as u128) as u64)
        .unwrap_or_default()
}

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_secs())
        .unwrap_or_default()
}

// ---- test seams ----

/// A point at which a test stops the closer as if it had been killed there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[cfg_attr(not(test), allow(dead_code))]
pub(crate) enum Stop {
    FoldPersisted,
    TermSent,
    ObligationOut,
    TerminalWritten,
    StartedDeleted,
}

/// A deliberately broken rule, for the falsification controls.
#[cfg(test)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Fault {
    SkipVeto,
    SkipSeal,
    SkipDescendantPersist,
    InferEmpty,
}

#[cfg(test)]
thread_local! {
    pub(crate) static STOP: std::cell::Cell<Option<Stop>> = const { std::cell::Cell::new(None) };
    pub(crate) static FAULT: std::cell::Cell<Option<Fault>> = const { std::cell::Cell::new(None) };
    pub(crate) static AFTER_FIRST_LISTING: std::cell::RefCell<Option<Box<dyn FnOnce()>>> =
        std::cell::RefCell::new(None);
}

#[cfg(test)]
fn fault(which: Fault) -> bool {
    FAULT.with(|fault| fault.get()) == Some(which)
}

fn stop_at(point: Stop) -> Result<(), ParentLossLedgerError> {
    #[cfg(test)]
    if STOP.with(|stop| stop.get()) == Some(point) {
        return Err(ParentLossLedgerError::RecoveryRequired(
            BootstrapRecoveryReason::CoordinatorNotLive,
        ));
    }
    #[cfg(feature = "test-hooks")]
    if point == Stop::TermSent
        && std::env::var("SOLSTONE_PARENT_LOSS_CLOSURE_SEAM").as_deref()
            == Ok("after-retirement-term")
    {
        loop {
            thread::sleep(Duration::from_millis(100));
        }
    }
    let _ = point;
    Ok(())
}

// ---- settlement ----

/// One identity's state, by exact observation and then the process table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Settlement {
    /// Proven gone: `Exited`, or `PidReusedByAnotherUser`.
    Gone(AdmissionFinding),
    Live,
    Unverifiable,
}

pub(crate) fn settle(
    source: &dyn ProcessInstanceSource,
    retirer: &dyn AdmissionRetirer,
    instance: ProcessInstance,
    uid: u32,
) -> Settlement {
    match source.observe(&instance) {
        InstanceVerdict::NotSameOrExited => Settlement::Gone(AdmissionFinding::Exited),
        InstanceVerdict::SameLive { .. } => Settlement::Live,
        InstanceVerdict::Unverifiable => {
            match settle_by_process_table(retirer, instance, Some(uid)) {
                Some(finding) => Settlement::Gone(finding),
                None => Settlement::Unverifiable,
            }
        }
    }
}

/// What a pid `inspect` cannot read may still be settled by: the process
/// table, which names every pid. Gone is exited; another user's is a reused
/// pid; our own user's with a different birth is a reused pid too, so the
/// remembered process has exited. Only that process itself, still live, or an
/// instrument failure settles nothing.
pub(crate) fn settle_by_process_table(
    retirer: &dyn AdmissionRetirer,
    instance: ProcessInstance,
    recorded_uid: Option<u32>,
) -> Option<AdmissionFinding> {
    match retirer.owner(instance.pid) {
        ProcessOwner::Absent => Some(AdmissionFinding::Exited),
        ProcessOwner::Uid(uid) if recorded_uid.is_some_and(|recorded| recorded != uid) => {
            Some(AdmissionFinding::PidReusedByAnotherUser)
        }
        ProcessOwner::Uid(_) => match retirer.table_birth(instance.pid) {
            TableBirth::Absent => Some(AdmissionFinding::Exited),
            TableBirth::Live(birth)
                if birth.is_verifiable()
                    && instance.birth.is_verifiable()
                    && birth != instance.birth =>
            {
                Some(AdmissionFinding::Exited)
            }
            TableBirth::Live(_) | TableBirth::Unknown => None,
        },
        ProcessOwner::Unknown => None,
    }
}

/// Re-observe a transient `Unverifiable` for a bounded window, so a pid that
/// exits between two procfs reads is not recorded as unobservable.
fn observe_settled(
    source: &dyn ProcessInstanceSource,
    instance: &ProcessInstance,
    deadline: Instant,
) -> InstanceVerdict {
    let settle = (Instant::now() + SETTLE_WINDOW).min(deadline);
    loop {
        let verdict = source.observe(instance);
        if verdict != InstanceVerdict::Unverifiable || Instant::now() >= settle {
            return verdict;
        }
        thread::sleep(POLL);
    }
}

/// The one veto: a recorded supervisor or coordinator still `SameLive`
/// refuses. The lease and the supervisor lock are the proof that neither
/// runs; this only refuses when the identity says otherwise.
pub(crate) fn authorities_veto(
    authority: &ClosingAuthority<'_>,
    supervisor: Option<ProcessInstance>,
    coordinator: Option<ProcessInstance>,
) -> Result<(AuthorityObservation, AuthorityObservation), BootstrapRecoveryReason> {
    let observe = |instance: Option<ProcessInstance>, live: BootstrapRecoveryReason| {
        #[cfg(test)]
        if fault(Fault::SkipVeto) {
            return Ok(AuthorityObservation::Exited);
        }
        match instance {
            None => Ok(AuthorityObservation::NeverRecorded),
            Some(instance) => {
                match observe_settled(authority.source, &instance, authority.deadline) {
                    InstanceVerdict::SameLive { .. } => Err(live),
                    InstanceVerdict::NotSameOrExited => Ok(AuthorityObservation::Exited),
                    InstanceVerdict::Unverifiable => Ok(AuthorityObservation::Unobservable),
                }
            }
        }
    };
    Ok((
        observe(supervisor, BootstrapRecoveryReason::SupervisorLive)?,
        observe(coordinator, BootstrapRecoveryReason::ActiveCoordinator)?,
    ))
}

// ---- the boot session ----

#[derive(Debug, Serialize, Deserialize)]
struct BootRecord {
    schema: u32,
    boot: String,
}

fn normalize_boot(text: &str) -> Option<String> {
    let text = text
        .trim()
        .trim_end_matches('\0')
        .trim()
        .to_ascii_lowercase();
    (!text.is_empty()).then_some(text)
}

/// The kernel's boot session id: Linux `boot_id`, macOS `kern.bootsessionuuid`.
pub(crate) fn current_boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .as_deref()
            .and_then(normalize_boot)
    }
    #[cfg(target_os = "macos")]
    {
        let mut buffer = [0u8; 64];
        let mut length = buffer.len();
        // SAFETY: the name is NUL-terminated and the buffer and its length
        // describe writable memory owned by this frame.
        let status = unsafe {
            libc::sysctlbyname(
                c"kern.bootsessionuuid".as_ptr(),
                buffer.as_mut_ptr().cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        if status != 0 {
            return None;
        }
        std::str::from_utf8(&buffer[..length.min(buffer.len())])
            .ok()
            .and_then(normalize_boot)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

fn boot_path(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> PathBuf {
    ledger.generation_path(generation).join("boot.json")
}

/// Record the boot a generation is reserved on. Written only from a
/// successful read; nothing is written when the id cannot be read.
pub(crate) fn record_boot(ledger: &ParentLossLedger, generation: ParentLossGeneration) {
    let Some(boot) = current_boot_id() else {
        return;
    };
    let path = boot_path(ledger, generation);
    if path
        .parent()
        .is_some_and(|dir| fs::create_dir_all(dir).is_ok())
    {
        let _ = write_json(
            &path,
            &BootRecord {
                schema: BOOT_SCHEMA_V1,
                boot,
            },
            json_options(),
        );
    }
}

/// Whether the generation provably ran on another boot. Both ids must be
/// present and readable and differ; anything else settles nothing.
fn boot_changed(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    current: Option<String>,
    supervisor: Option<ProcessInstance>,
) -> bool {
    let Some(current) = current.as_deref().and_then(normalize_boot) else {
        return false;
    };
    let recorded = fs::read(boot_path(ledger, generation))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BootRecord>(&bytes).ok())
        .filter(|record| record.schema == BOOT_SCHEMA_V1)
        .and_then(|record| normalize_boot(&record.boot));
    if recorded.is_some_and(|recorded| recorded != current) {
        return true;
    }
    // A Linux supervisor birth carries the boot it was observed on.
    match (
        supervisor.and_then(|supervisor| supervisor.birth.linux_boot()),
        parse_linux_boot_id(&current),
    ) {
        (Some(recorded), Some(current)) => recorded != current,
        _ => false,
    }
}

// ---- the tolerant entry reader ----

/// One outstanding launch as its drops describe it. Each drop is parsed on
/// its own; a torn, unparseable or other-schema drop is reported and yields
/// nothing, and the rest still count.
#[derive(Debug, Clone, Default)]
pub(crate) struct OutstandingEntry {
    pub(crate) launch_id: String,
    pub(crate) service: Option<HostedServiceKind>,
    pub(crate) parent_launch_id: Option<String>,
    /// Every identity any drop yields, in the released reader's preference
    /// order (result, acknowledgement, spawn identity), without repeats.
    pub(crate) identities: Vec<AdmissionIdentity>,
    pub(crate) result: Option<AdmissionResultState>,
    pub(crate) unreadable: Vec<&'static str>,
    pub(crate) sources: AdmissionSourceSnapshot,
}

impl OutstandingEntry {
    /// The released reader's classification of an entry with no identity.
    pub(crate) fn without_identity(&self) -> AdmissionFinding {
        match &self.result {
            Some(AdmissionResultState::SpawnFailed { .. }) => AdmissionFinding::SpawnFailed,
            Some(AdmissionResultState::RejectedAndReaped { .. }) => {
                AdmissionFinding::RejectedAndReaped
            }
            Some(_) => AdmissionFinding::Unidentified,
            None => AdmissionFinding::NeverAcknowledged,
        }
    }

    /// Whether the entry needs no process observed: it never had one.
    pub(crate) fn settled_without_process(&self) -> bool {
        self.identities.is_empty()
            && matches!(
                self.result,
                Some(
                    AdmissionResultState::SpawnFailed { .. }
                        | AdmissionResultState::RejectedAndReaped { .. }
                )
            )
    }
}

fn read_drop<T: DeserializeOwned>(
    directory: &Path,
    name: &'static str,
    schema: fn(&T) -> u32,
    text: &mut Option<String>,
    unreadable: &mut Vec<&'static str>,
) -> io::Result<Option<T>> {
    let bytes = match fs::read(directory.join(name)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    // Drops are created and then filled; a writer killed between the two
    // leaves an empty file, which is the drop that was never written.
    if bytes.is_empty() {
        return Ok(None);
    }
    match serde_json::from_slice::<T>(&bytes) {
        Ok(value) if schema(&value) == 1 => {
            *text = String::from_utf8(bytes).ok();
            Ok(Some(value))
        }
        _ => {
            *text = String::from_utf8(bytes).ok();
            unreadable.push(name);
            Ok(None)
        }
    }
}

/// Read one entry. Only an I/O failure other than absence is an error: the
/// drop is there and cannot be read, so nothing about it can be settled.
pub(crate) fn read_entry(admissions: &Path, launch_id: &str) -> io::Result<OutstandingEntry> {
    let directory = admissions.join(launch_id);
    let mut entry = OutstandingEntry {
        launch_id: launch_id.to_owned(),
        ..OutstandingEntry::default()
    };
    let mut sources = AdmissionSourceSnapshot::default();
    let mut unreadable = Vec::new();
    let intent: Option<AdmissionIntent> = read_drop(
        &directory,
        "intent.json",
        |v: &AdmissionIntent| v.schema,
        &mut sources.intent,
        &mut unreadable,
    )?;
    let result: Option<AdmissionResult> = read_drop(
        &directory,
        "result.json",
        |v: &AdmissionResult| v.schema,
        &mut sources.result,
        &mut unreadable,
    )?;
    let acknowledgement: Option<AdmissionAcknowledgement> = read_drop(
        &directory,
        "acknowledgement.json",
        |v: &AdmissionAcknowledgement| v.schema,
        &mut sources.acknowledgement,
        &mut unreadable,
    )?;
    let spawn: Option<AdmissionSpawnIdentity> = read_drop(
        &directory,
        "spawn-identity.json",
        |v: &AdmissionSpawnIdentity| v.schema,
        &mut sources.spawn_identity,
        &mut unreadable,
    )?;
    let mut identities: Vec<AdmissionIdentity> = Vec::new();
    let found = [
        result.as_ref().and_then(|result| result.identity.clone()),
        acknowledgement.map(|ack| ack.identity),
        spawn.map(|spawn| spawn.identity),
    ];
    for identity in found.into_iter().flatten() {
        if !identities
            .iter()
            .any(|known| known.instance == identity.instance && known.uid == identity.uid)
        {
            identities.push(identity);
        }
    }
    entry.service = intent.as_ref().and_then(|intent| intent.service);
    entry.parent_launch_id = identities
        .first()
        .and_then(|identity| identity.parent_launch_id.clone())
        .or_else(|| intent.and_then(|intent| intent.parent_launch_id));
    entry.identities = identities;
    entry.result = result.map(|result| result.state);
    entry.unreadable = unreadable;
    entry.sources = sources;
    Ok(entry)
}

/// The entry names of one `admissions/` listing, read once into memory, and
/// what was skipped. `None` when the directory does not exist.
#[derive(Debug, Default)]
pub(crate) struct Listing {
    pub(crate) names: Vec<String>,
    pub(crate) skipped: Vec<String>,
}

pub(crate) fn list_entries(admissions: &Path) -> io::Result<Option<Listing>> {
    let children = match fs::read_dir(admissions) {
        Ok(children) => children,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    let mut listing = Listing::default();
    for child in children {
        let child = child?;
        let name = child.file_name();
        match name.to_str() {
            Some(name) if validate_launch_id(name).is_ok() && child.file_type()?.is_dir() => {
                listing.names.push(name.to_owned());
            }
            _ => listing.skipped.push(name.to_string_lossy().into_owned()),
        }
    }
    listing.names.sort_unstable();
    Ok(Some(listing))
}

/// Read a window of entries with a few threads; reads are independent.
pub(crate) fn read_entries(
    admissions: &Path,
    names: &[String],
) -> Vec<io::Result<OutstandingEntry>> {
    let threads = thread::available_parallelism()
        .map_or(1, |n| n.get())
        .clamp(1, READ_THREADS);
    let chunk = names.len().div_ceil(threads).max(1);
    thread::scope(|scope| {
        let readers: Vec<_> = names
            .chunks(chunk)
            .map(|part| {
                scope.spawn(move || {
                    part.iter()
                        .map(|name| read_entry(admissions, name))
                        .collect::<Vec<_>>()
                })
            })
            .collect();
        readers
            .into_iter()
            .flat_map(|reader| {
                reader
                    .join()
                    .unwrap_or_else(|panic| std::panic::resume_unwind(panic))
            })
            .collect()
    })
}

// ---- discharge ----

fn discharged_dir(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> PathBuf {
    ledger.generation_path(generation).join("discharged")
}

fn admission_lock(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    timeout: Duration,
) -> Result<solstone_core_journal_io::FileLock, solstone_core_journal_io::LockError> {
    hold_lock(
        ledger.admission_lock_path(generation),
        LockOptions {
            timeout,
            poll_interval: Duration::from_millis(10),
            mode: Some(0o600),
        },
    )
}

fn unique_suffix() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|elapsed| elapsed.as_nanos())
        .unwrap_or_default()
}

/// Move an entry out of the outstanding set, under the admission lock. A
/// rename, no fsync: a crash only re-observes it, and dead stays dead
/// because identity is exact. An entry already gone is already discharged.
// replaced by admission::discharge at merge
pub(crate) fn discharge_sealed(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<(), ParentLossLedgerError> {
    let _lock = admission_lock(ledger, generation, DISCHARGE_LOCK_TIMEOUT)?;
    let from = admission_directory(ledger, generation).join(launch_id);
    let trash = discharged_dir(ledger, generation);
    fs::create_dir_all(&trash)?;
    match fs::rename(&from, trash.join(launch_id)) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(_) => {
            // The trash already holds this name: keep both.
            match fs::rename(
                &from,
                trash.join(format!("{launch_id}.{}", unique_suffix())),
            ) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error.into()),
            }
        }
    }
}

/// Move a bookkeeping file or tree into the trash. The name carries a `.`,
/// which no launch id does, so it never collides with an entry.
fn move_to_discharged(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    path: &Path,
) -> io::Result<()> {
    let trash = discharged_dir(ledger, generation);
    fs::create_dir_all(&trash)?;
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    match fs::rename(path, trash.join(format!("{name}.{}", unique_suffix()))) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

// ---- the census: read, fold, persist ----

fn obligation_keys_valid(file: &CensusObligationFile) -> bool {
    file.obligations
        .keys()
        .all(|id| validate_launch_id(id).is_ok())
}

/// What a generation's descendant census evidence says, read without side
/// effects. `lost` is the one refusal the closer keeps: retirement began and
/// no authoritative census parses completely.
struct CensusEvidence {
    main: Option<CensusObligationFile>,
    main_damaged: bool,
    copies: Vec<CensusObligationFile>,
    markers: Vec<PathBuf>,
    lost: bool,
}

fn census_evidence(generation_dir: &Path) -> io::Result<CensusEvidence> {
    let main_path = generation_dir.join("census-obligation.json");
    let (main, main_damaged) = match read_json_durable::<CensusObligationFile>(
        ArtifactId::ParentLossCensusObligation,
        &main_path,
    ) {
        Ok(DurableRead::Present(file)) if obligation_keys_valid(&file) => (Some(file), false),
        Ok(DurableRead::Absent) => (None, false),
        Ok(_) => (None, true),
        Err(error) if error.kind() == io::ErrorKind::InvalidData => (None, true),
        Err(error) => return Err(error),
    };
    let started = generation_dir.join("census-started.json").exists();
    let mut copies = Vec::new();
    let mut census_copies = 0;
    let mut markers = Vec::new();
    match fs::read_dir(generation_dir) {
        Ok(children) => {
            for child in children {
                let child = child?;
                let name = child.file_name().to_string_lossy().into_owned();
                if name.starts_with("census-obligation.wedged-") {
                    census_copies += 1;
                    if let Some(file) = fs::read(child.path())
                        .ok()
                        .and_then(|bytes| {
                            serde_json::from_slice::<CensusObligationFile>(&bytes).ok()
                        })
                        .filter(obligation_keys_valid)
                    {
                        copies.push(file);
                    }
                    markers.push(child.path());
                } else if legacy_refusal_marker(&name) {
                    markers.push(child.path());
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    // With nothing ever written, the released binary's own reader reads the
    // empty census as authoritative; so does this one.
    let never_written = main.is_none() && !main_damaged && !started && census_copies == 0;
    let lost = main.is_none() && copies.is_empty() && !never_written;
    Ok(CensusEvidence {
        main,
        main_damaged,
        copies,
        markers,
        lost,
    })
}

fn legacy_refusal_marker(name: &str) -> bool {
    name == "census-refusal.json" || name.starts_with("census-refusal.wedged-")
}

/// Whether a generation directory holds a lost descendant census: a
/// retirement began and no authoritative census parses completely.
pub fn parent_loss_census_is_lost(generation_dir: &Path) -> bool {
    census_evidence(generation_dir).is_ok_and(|evidence| evidence.lost)
}

fn timing_rank(live: &LiveRecoveryFile) -> (bool, bool) {
    (live.escalated, live.term_requested)
}

fn merge_descendant(into: &mut Vec<LiveDescendantIdentity>, other: LiveDescendantIdentity) {
    match into
        .iter_mut()
        .find(|known| known.instance == other.instance)
    {
        Some(known) => {
            known.term_requested |= other.term_requested;
            known.escalated |= other.escalated;
            if other.escalate_at_unix_ms != 0
                && (known.escalate_at_unix_ms == 0
                    || other.escalate_at_unix_ms < known.escalate_at_unix_ms)
            {
                known.escalate_at_unix_ms = other.escalate_at_unix_ms;
            }
            known.reap_deadline_unix_ms =
                match (known.reap_deadline_unix_ms, other.reap_deadline_unix_ms) {
                    (Some(a), Some(b)) => Some(a.min(b)),
                    (a, b) => a.or(b),
                };
        }
        None => into.push(other),
    }
}

/// Merge two records of one launch's obligation: prefer the one whose
/// retirement went further, then the earlier escalation, and keep every
/// identity either names.
fn merge_obligation(into: &mut Obligations, launch_id: String, other: LiveRecoveryFile) {
    let Some(known) = into.get_mut(&launch_id) else {
        into.insert(launch_id, other);
        return;
    };
    let other_first = timing_rank(&other) > timing_rank(known)
        || (timing_rank(&other) == timing_rank(known)
            && other.escalate_at_unix_ms != 0
            && (known.escalate_at_unix_ms == 0
                || other.escalate_at_unix_ms < known.escalate_at_unix_ms));
    let (mut base, rest) = if other_first {
        (other, known.clone())
    } else {
        (known.clone(), other)
    };
    if base.sources.is_none() {
        base.sources = rest.sources.clone();
    }
    if rest.root != base.root {
        merge_descendant(&mut base.descendants, as_descendant(&rest));
    }
    for descendant in rest.descendants {
        if descendant.instance != base.root {
            merge_descendant(&mut base.descendants, descendant);
        }
    }
    *known = base;
}

fn as_descendant(live: &LiveRecoveryFile) -> LiveDescendantIdentity {
    LiveDescendantIdentity {
        instance: live.root,
        uid: live.uid,
        ppid: 0,
        pgid: 0,
        escalate_at_unix_ms: live.escalate_at_unix_ms,
        reap_deadline_unix_ms: live.reap_deadline_unix_ms,
        term_requested: live.term_requested,
        escalated: live.escalated,
    }
}

/// Write the census the released binary reads: the obligation file first,
/// then the marker that retirement began.
fn persist_census(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    obligations: &Obligations,
) -> Result<(), ParentLossLedgerError> {
    let mut obligations = obligations.clone();
    for (launch_id, live) in obligations.iter_mut() {
        live.schema = RECOVERY_SCHEMA_V1;
        live.kind = RecoveryArtifactKind::Live;
        live.generation = generation;
        live.sealed_phase = ParentLossPhase::Sealed;
        live.launch_id = launch_id.clone();
        live.binding_digest = compute_live_binding_digest(live.root, live.uid, &live.descendants)?;
    }
    let file = CensusObligationFile {
        schema: RECOVERY_SCHEMA_V1,
        generation,
        sealed_phase: ParentLossPhase::Sealed,
        binding_digest: compute_census_obligation_digest(&obligations)?,
        obligations,
    };
    write_json(
        ledger.census_obligation_path(generation),
        &file,
        json_options(),
    )?;
    let started = ledger
        .generation_path(generation)
        .join("census-started.json");
    if !file.obligations.is_empty() && !started.exists() {
        write_json(
            started,
            &serde_json::json!({"schema": 1, "generation": generation}),
            json_options(),
        )?;
    }
    Ok(())
}

/// Gather every descendant obligation a generation's earlier retirements
/// left: the census, its set-aside copies, and the released closer's
/// `closure-recovery/live/` cache, which only adds to an authoritative census
/// and never stands in for one. The merged census is persisted before any
/// legacy cache is moved into the trash. `None` is a lost census.
fn fold_census(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    notes: &mut Vec<String>,
) -> Result<Option<Obligations>, ParentLossLedgerError> {
    let generation_dir = ledger.generation_path(generation);
    let evidence = census_evidence(&generation_dir)?;
    if evidence.lost {
        return Ok(None);
    }
    let mut merged = Obligations::new();
    let main = evidence.main.map(|file| file.obligations);
    for (launch_id, live) in main.iter().flatten() {
        merge_obligation(&mut merged, launch_id.clone(), live.clone());
    }
    for copy in evidence.copies {
        for (launch_id, live) in copy.obligations {
            merge_obligation(&mut merged, launch_id, live);
        }
    }
    let recovery = ledger.closure_recovery_dir(generation);
    if let Ok(children) = fs::read_dir(recovery.join("live")) {
        for child in children.flatten() {
            let path = child.path();
            let Some(launch_id) = path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .filter(|stem| validate_launch_id(stem).is_ok())
                .map(str::to_owned)
            else {
                continue;
            };
            if let Ok(DurableRead::Present(live)) = read_json_durable_validated::<LiveRecoveryFile>(
                ArtifactId::ParentLossClosureRecovery,
                &path,
                |_| Ok(()),
            ) {
                merge_obligation(&mut merged, launch_id, live);
            }
        }
    }
    // Every obligation carries the drops its root came from, so a released
    // binary can still authorize it.
    let admissions = admission_directory(ledger, generation);
    for (launch_id, live) in merged.iter_mut() {
        if live.sources.is_none()
            && let Ok(entry) = read_entry(&admissions, launch_id)
            && entry.sources != AdmissionSourceSnapshot::default()
        {
            live.sources = Some(entry.sources);
        }
    }
    let mut caches: Vec<PathBuf> = [recovery, ledger.closure_recovery_legacy_path(generation)]
        .into_iter()
        .filter(|path| path.exists())
        .collect();
    caches.extend(evidence.markers);
    if evidence.main_damaged || !caches.is_empty() || main.as_ref() != Some(&merged) {
        if !caches.is_empty() || evidence.main_damaged || !merged.is_empty() {
            persist_census(ledger, generation, &merged)?;
            stop_at(Stop::FoldPersisted)?;
        }
        if !caches.is_empty() {
            notes.push(format!(
                "{} legacy recovery file(s) were folded into the census and discarded",
                caches.len()
            ));
        }
        for cache in caches {
            move_to_discharged(ledger, generation, &cache)?;
        }
    }
    Ok(Some(merged))
}

// ---- retirement ----

#[derive(Debug, Clone, Copy)]
struct Timing {
    escalate_at: u64,
    reap_deadline: Option<u64>,
    term_requested: bool,
    escalated: bool,
}

/// One identity an obligation names: its root (`None`) or a descendant.
type Target = (String, Option<usize>);

fn target_identity(live: &LiveRecoveryFile, which: Option<usize>) -> (ProcessInstance, u32) {
    match which {
        None => (live.root, live.uid),
        Some(index) => (
            live.descendants[index].instance,
            live.descendants[index].uid,
        ),
    }
}

fn timing(live: &LiveRecoveryFile, which: Option<usize>) -> Timing {
    match which {
        None => Timing {
            escalate_at: live.escalate_at_unix_ms,
            reap_deadline: live.reap_deadline_unix_ms,
            term_requested: live.term_requested,
            escalated: live.escalated,
        },
        Some(index) => {
            let d = &live.descendants[index];
            Timing {
                escalate_at: d.escalate_at_unix_ms,
                reap_deadline: d.reap_deadline_unix_ms,
                term_requested: d.term_requested,
                escalated: d.escalated,
            }
        }
    }
}

fn set_timing(live: &mut LiveRecoveryFile, which: Option<usize>, value: Timing) {
    match which {
        None => {
            live.escalate_at_unix_ms = value.escalate_at;
            live.reap_deadline_unix_ms = value.reap_deadline;
            live.term_requested = value.term_requested;
            live.escalated = value.escalated;
        }
        Some(index) => {
            let d = &mut live.descendants[index];
            d.escalate_at_unix_ms = value.escalate_at;
            d.reap_deadline_unix_ms = value.reap_deadline;
            d.term_requested = value.term_requested;
            d.escalated = value.escalated;
        }
    }
}

#[cfg(any(target_os = "linux", target_os = "macos"))]
fn snapshot_descendants(
    root: ProcessInstance,
    uid: u32,
    source: &dyn ProcessInstanceSource,
) -> Option<Vec<LiveDescendantIdentity>> {
    let snapshot = exact_descendant_tree(root, uid, source).ok()?;
    Some(
        snapshot
            .descendants
            .into_iter()
            .filter_map(|d| {
                let birth = snapshot.descendant_births.get(&d.pid).copied()?;
                Some(LiveDescendantIdentity {
                    instance: ProcessInstance {
                        pid: u32::try_from(d.pid).ok()?,
                        birth,
                    },
                    uid: d.uid,
                    ppid: u32::try_from(d.ppid).unwrap_or(0),
                    pgid: d.pgid.and_then(|g| u32::try_from(g).ok()).unwrap_or(0),
                    escalate_at_unix_ms: 0,
                    reap_deadline_unix_ms: None,
                    term_requested: false,
                    escalated: false,
                })
            })
            .collect(),
    )
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn snapshot_descendants(
    _root: ProcessInstance,
    _uid: u32,
    _source: &dyn ProcessInstanceSource,
) -> Option<Vec<LiveDescendantIdentity>> {
    Some(Vec::new())
}

#[derive(Debug, Default)]
struct Retirement {
    /// Obligations whose root and every descendant are proven gone.
    settled: Vec<(String, AdmissionFinding)>,
    live: usize,
    unverifiable: usize,
}

/// Retire every obligation at once against one absolute deadline.
///
/// Every live root's descendants are snapshotted and the whole schedule is
/// persisted before any signal. Then TERM goes to every live identity at
/// once, KILL to each at its persisted `escalate_at`, and each is given up on
/// at its `reap_deadline`. Before each signal the identity is observed live
/// and its owner checked. An obligation whose descendants cannot be read, or
/// that names another user's process, is never signalled.
fn retire(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    obligations: &mut Obligations,
    authority: &ClosingAuthority<'_>,
) -> Result<Retirement, ParentLossLedgerError> {
    let source = authority.source;
    let retirer = authority.retirer;
    let mut blind: HashSet<String> = HashSet::new();
    for (launch_id, live) in obligations.iter_mut() {
        if matches!(source.observe(&live.root), InstanceVerdict::SameLive { .. }) {
            match snapshot_descendants(live.root, live.uid, source) {
                Some(found) => {
                    for descendant in found {
                        if descendant.instance != live.root
                            && !live
                                .descendants
                                .iter()
                                .any(|known| known.instance == descendant.instance)
                        {
                            live.descendants.push(descendant);
                        }
                    }
                }
                None => {
                    blind.insert(launch_id.clone());
                }
            }
        }
    }
    let mut targets: Vec<Target> = Vec::new();
    for (launch_id, live) in obligations.iter() {
        targets.push((launch_id.clone(), None));
        targets.extend((0..live.descendants.len()).map(|index| (launch_id.clone(), Some(index))));
    }
    let observe = |obligations: &Obligations, (launch_id, which): &Target| {
        let (instance, uid) = target_identity(&obligations[launch_id], *which);
        settle(source, retirer, instance, uid)
    };
    let mut state: HashMap<Target, Settlement> = targets
        .iter()
        .map(|target| (target.clone(), observe(obligations, target)))
        .collect();
    // An obligation naming another user's live process is never signalled.
    let foreign: HashSet<String> = targets
        .iter()
        .filter(|target| state[*target] == Settlement::Live)
        .filter(|(launch_id, which)| {
            let live = &obligations[launch_id];
            let (instance, uid) = target_identity(live, *which);
            uid != live.uid || retirer.owner(instance.pid) != ProcessOwner::Uid(uid)
        })
        .map(|(launch_id, _)| launch_id.clone())
        .collect();
    let signalled = |launch_id: &String| !blind.contains(launch_id) && !foreign.contains(launch_id);

    let now_ms = unix_millis();
    let mut any_live = false;
    for target in &targets {
        if state[target] != Settlement::Live || !signalled(&target.0) {
            continue;
        }
        any_live = true;
        let live = obligations.get_mut(&target.0).expect("target obligation");
        let mut t = timing(live, target.1);
        if !t.term_requested {
            t.term_requested = true;
            t.escalate_at = now_ms + TERM_GRACE_MS;
        } else if t.escalate_at == 0 || t.escalate_at > now_ms + TERM_GRACE_MS {
            t.escalate_at = now_ms + TERM_GRACE_MS;
        }
        if t.reap_deadline
            .is_some_and(|reap| reap > now_ms + REAP_GRACE_MS)
        {
            t.reap_deadline = Some(now_ms + REAP_GRACE_MS);
        }
        set_timing(live, target.1, t);
    }
    // ⛔ Descendants and schedule are durable before the first signal.
    #[cfg(test)]
    if fault(Fault::SkipDescendantPersist) {
        let mut without = obligations.clone();
        without
            .values_mut()
            .for_each(|live| live.descendants.clear());
        persist_census(ledger, generation, &without)?;
    } else {
        persist_census(ledger, generation, obligations)?;
    }
    #[cfg(not(test))]
    persist_census(ledger, generation, obligations)?;

    #[cfg(any(test, feature = "test-hooks"))]
    if any_live && crate::process::retirement_skip_test_fault() {
        return Ok(Retirement {
            live: 1,
            ..Retirement::default()
        });
    }
    let _ = any_live;

    for target in &targets {
        if state[target] == Settlement::Live && signalled(&target.0) {
            let (instance, uid) = target_identity(&obligations[&target.0], target.1);
            if retirer.owner(instance.pid) == ProcessOwner::Uid(uid) {
                let _ = retirer.signal(instance, SignalKind::Terminate);
            }
        }
    }
    stop_at(Stop::TermSent)?;

    loop {
        let now_ms = unix_millis();
        let mut waiting = false;
        let mut due: Vec<Target> = Vec::new();
        for target in &targets {
            if matches!(state[target], Settlement::Gone(_)) {
                continue;
            }
            let observed = observe(obligations, target);
            state.insert(target.clone(), observed);
            if observed != Settlement::Live || !signalled(&target.0) {
                continue;
            }
            let t = timing(&obligations[&target.0], target.1);
            if now_ms >= t.escalate_at && (!t.escalated || t.reap_deadline.is_none()) {
                due.push(target.clone());
                waiting = true;
            } else if !t.reap_deadline.is_some_and(|reap| now_ms >= reap) {
                waiting = true;
            }
        }
        if !due.is_empty() {
            for target in &due {
                let live = obligations.get_mut(&target.0).expect("target obligation");
                let mut t = timing(live, target.1);
                t.escalated = true;
                set_timing(live, target.1, t);
            }
            persist_census(ledger, generation, obligations)?;
            for target in &due {
                let (instance, uid) = target_identity(&obligations[&target.0], target.1);
                if matches!(source.observe(&instance), InstanceVerdict::SameLive { .. })
                    && retirer.owner(instance.pid) == ProcessOwner::Uid(uid)
                {
                    let _ = retirer.signal(instance, SignalKind::Kill);
                }
            }
            // The reap grace starts once the kills are sent, and a stored
            // deadline only ever moves earlier.
            let reap = unix_millis() + REAP_GRACE_MS;
            for target in &due {
                let live = obligations.get_mut(&target.0).expect("target obligation");
                let mut t = timing(live, target.1);
                t.reap_deadline = Some(t.reap_deadline.map_or(reap, |stored| stored.min(reap)));
                set_timing(live, target.1, t);
            }
            persist_census(ledger, generation, obligations)?;
            continue;
        }
        if !waiting || Instant::now() >= authority.deadline {
            break;
        }
        thread::sleep(POLL.min(authority.deadline.saturating_duration_since(Instant::now())));
    }

    let mut retirement = Retirement::default();
    for (launch_id, live) in obligations.iter() {
        let mine: Vec<Settlement> = targets
            .iter()
            .filter(|(id, _)| id == launch_id)
            .map(|target| state[target])
            .collect();
        if blind.contains(launch_id) {
            retirement.unverifiable += 1;
        } else if mine.iter().all(|s| matches!(s, Settlement::Gone(_))) {
            let signalled_any =
                live.term_requested || live.descendants.iter().any(|d| d.term_requested);
            let finding = if signalled_any {
                AdmissionFinding::Retired {
                    escalated: live.escalated || live.descendants.iter().any(|d| d.escalated),
                }
            } else {
                match state[&(launch_id.clone(), None)] {
                    Settlement::Gone(finding) => finding,
                    _ => AdmissionFinding::Exited,
                }
            };
            retirement.settled.push((launch_id.clone(), finding));
        } else if mine.contains(&Settlement::Live) {
            retirement.live += 1;
        } else {
            retirement.unverifiable += 1;
        }
    }
    Ok(retirement)
}

// ---- the closer ----

/// What a closure found, row by row and in total.
#[derive(Default)]
struct Tally {
    summary: ClosureSummary,
    rows: Vec<AdmissionClosure>,
    omitted: u64,
}

impl Tally {
    fn record(
        &mut self,
        launch_id: &str,
        service: Option<HostedServiceKind>,
        identity: Option<(ProcessInstance, u32)>,
        finding: AdmissionFinding,
    ) {
        self.summary.count(finding);
        let noteworthy = matches!(
            finding,
            AdmissionFinding::Retired { .. }
                | AdmissionFinding::NeverAcknowledged
                | AdmissionFinding::Unidentified
        );
        if !noteworthy {
            return;
        }
        if self.rows.len() >= NOTEWORTHY_ROW_LIMIT {
            self.omitted += 1;
            return;
        }
        self.rows.push(AdmissionClosure {
            launch_id: launch_id.to_owned(),
            service,
            instance: identity.map(|(instance, _)| instance),
            uid: identity.map(|(_, uid)| uid),
            finding,
        });
    }
}

/// Read a generation's record for closure. A record that cannot be parsed or
/// names another generation is bookkeeping we cannot trust and is set aside
/// beside itself; the admissions directory is the evidence that survives it.
fn load_record_for_closure(
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
        Ok(DurableRead::SetAside(aside)) => Ok((
            None,
            aside
                .file_name()
                .map(|name| vec![name.to_string_lossy().into_owned()])
                .unwrap_or_default(),
        )),
        Ok(DurableRead::Absent | DurableRead::Unreadable { .. }) => Ok((None, Vec::new())),
        Err(error) => Err(error.into()),
    }
}

/// Seal the pointer under the admission lock, so no launch admits after
/// this. `false` when a live launcher holds the lock past the wait.
fn seal_pointer(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> Result<bool, ParentLossLedgerError> {
    #[cfg(test)]
    if fault(Fault::SkipSeal) {
        return Ok(true);
    }
    let Ok(_lock) = admission_lock(ledger, generation, ADMISSION_SEAL_LOCK_TIMEOUT) else {
        return Ok(false);
    };
    ledger.mark_sealed_for_closure(generation)?;
    Ok(true)
}

/// The obligation a live entry becomes: its first live identity is the root,
/// any other live identity a descendant, all retired by exact identity.
fn hold_live(
    obligations: &mut Obligations,
    generation: ParentLossGeneration,
    entry: &OutstandingEntry,
    live: Vec<AdmissionIdentity>,
) {
    let descendant = |identity: &AdmissionIdentity| LiveDescendantIdentity {
        instance: identity.instance,
        uid: identity.uid,
        ppid: 0,
        pgid: 0,
        escalate_at_unix_ms: 0,
        reap_deadline_unix_ms: None,
        term_requested: false,
        escalated: false,
    };
    if let Some(known) = obligations.get_mut(&entry.launch_id) {
        for identity in &live {
            if known.root != identity.instance
                && !known
                    .descendants
                    .iter()
                    .any(|d| d.instance == identity.instance)
            {
                known.descendants.push(descendant(identity));
            }
        }
        if known.sources.is_none() {
            known.sources = Some(entry.sources.clone());
        }
        return;
    }
    let mut live = live.into_iter();
    let Some(root) = live.next() else {
        return;
    };
    obligations.insert(
        entry.launch_id.clone(),
        LiveRecoveryFile {
            sources: Some(entry.sources.clone()),
            schema: RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: entry.launch_id.clone(),
            service: entry.service,
            root: root.instance,
            uid: root.uid,
            parent_launch_id: entry.parent_launch_id.clone(),
            descendants: live.map(|identity| descendant(&identity)).collect(),
            escalate_at_unix_ms: 0,
            reap_deadline_unix_ms: None,
            term_requested: false,
            escalated: false,
            binding_digest: String::new(),
        },
    );
}

/// Settle one listing: observe every entry, discharge what is proven gone,
/// then retire every obligation at once. `Some` is a refusal.
fn settle_listing(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    names: &[String],
    obligations: &mut Obligations,
    tally: &mut Tally,
    notes: &mut Vec<String>,
    authority: &ClosingAuthority<'_>,
) -> Result<Option<BootstrapRecoveryReason>, ParentLossLedgerError> {
    let admissions = admission_directory(ledger, generation);
    let mut held: HashMap<String, OutstandingEntry> = HashMap::new();
    let mut unverifiable = 0usize;
    for window in names.chunks(READ_WINDOW) {
        if Instant::now() >= authority.deadline {
            return Ok(Some(BootstrapRecoveryReason::CoordinatorNotLive));
        }
        for entry in read_entries(&admissions, window) {
            let Ok(entry) = entry else {
                unverifiable += 1;
                continue;
            };
            if !entry.unreadable.is_empty() {
                notes.push(format!(
                    "admission {}: unreadable {}",
                    entry.launch_id,
                    entry.unreadable.join(", ")
                ));
            }
            let mut live = Vec::new();
            let mut blind = false;
            let mut reused = false;
            for identity in &entry.identities {
                match settle(
                    authority.source,
                    authority.retirer,
                    identity.instance,
                    identity.uid,
                ) {
                    Settlement::Gone(AdmissionFinding::PidReusedByAnotherUser) => reused = true,
                    Settlement::Gone(_) => {}
                    Settlement::Live => live.push(identity.clone()),
                    Settlement::Unverifiable => blind = true,
                }
            }
            hold_live(obligations, generation, &entry, live);
            if blind {
                unverifiable += 1;
                continue;
            }
            if obligations.contains_key(&entry.launch_id) {
                held.insert(entry.launch_id.clone(), entry);
                continue;
            }
            let finding = if entry.identities.is_empty() {
                entry.without_identity()
            } else if reused {
                AdmissionFinding::PidReusedByAnotherUser
            } else {
                AdmissionFinding::Exited
            };
            discharge_sealed(ledger, generation, &entry.launch_id)?;
            tally.record(
                &entry.launch_id,
                entry.service,
                entry.identities.first().map(|id| (id.instance, id.uid)),
                finding,
            );
        }
    }
    if !obligations.is_empty() {
        let retirement = retire(ledger, generation, obligations, authority)?;
        if !retirement.settled.is_empty() {
            let mut out = Vec::new();
            for (launch_id, finding) in retirement.settled {
                if let Some(live) = obligations.remove(&launch_id) {
                    out.push((launch_id, finding, live));
                }
            }
            // ⛔ An obligation leaves the census, durably, before its entry.
            persist_census(ledger, generation, obligations)?;
            stop_at(Stop::ObligationOut)?;
            for (launch_id, finding, live) in out {
                if let Some(entry) = held.remove(&launch_id) {
                    discharge_sealed(ledger, generation, &launch_id)?;
                    tally.record(
                        &launch_id,
                        entry.service,
                        Some((live.root, live.uid)),
                        finding,
                    );
                } else {
                    tally.record(
                        &launch_id,
                        live.service,
                        Some((live.root, live.uid)),
                        finding,
                    );
                }
            }
        }
        if retirement.live > 0 {
            return Ok(Some(BootstrapRecoveryReason::AbandonedAdmissionLive));
        }
        unverifiable += retirement.unverifiable;
    }
    if unverifiable > 0 {
        return Ok(Some(
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
        ));
    }
    Ok(None)
}

/// Close one generation whose supervisor and coordinator are both gone. The
/// caller holds the active-generation lock and the coordinator lease, and its
/// parent holds the supervisor lock. `pointer` is the active pointer when it
/// names this generation.
pub(crate) fn close_generation(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    pointer: Option<&ActiveGeneration>,
    mut notes: Vec<String>,
    successor_generation: ParentLossGeneration,
    authority: &ClosingAuthority<'_>,
) -> Result<ClosureOutcome, ParentLossLedgerError> {
    let (record, set_aside) = load_record_for_closure(ledger, generation)?;
    let supervisor = pointer
        .map(|pointer| pointer.supervisor)
        .or_else(|| record.as_ref().map(|record| record.supervisor));
    let coordinator = pointer
        .and_then(|pointer| pointer.coordinator)
        .or_else(|| record.as_ref().and_then(|record| record.coordinator));
    let observations = match authorities_veto(authority, supervisor, coordinator) {
        Ok(observations) => observations,
        Err(reason) => return Ok(ClosureOutcome::Refused(reason)),
    };
    if pointer.is_some() && !seal_pointer(ledger, generation)? {
        return Ok(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionLive,
        ));
    }
    let mut tally = Tally::default();
    let close = Close {
        ledger,
        generation,
        successor_generation,
        authority,
        supervisor,
        coordinator,
        observations,
    };
    if boot_changed(ledger, generation, authority.retirer.boot_id(), supervisor) {
        tally.summary.boot_changed = true;
        notes.push(
            "the computer restarted since this run, so nothing it started can still be running"
                .to_owned(),
        );
        let generation_dir = ledger.generation_path(generation);
        let mut caches: Vec<PathBuf> = [
            ledger.closure_recovery_dir(generation),
            ledger.closure_recovery_legacy_path(generation),
        ]
        .into_iter()
        .filter(|path| path.exists())
        .collect();
        if let Ok(evidence) = census_evidence(&generation_dir) {
            caches.extend(evidence.markers);
        }
        close.finish(record, set_aside, notes, tally)?;
        for cache in caches {
            move_to_discharged(ledger, generation, &cache)?;
        }
        return Ok(ClosureOutcome::Closed);
    }
    let Some(mut obligations) = fold_census(ledger, generation, &mut notes)? else {
        return Ok(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
        ));
    };
    let admissions = admission_directory(ledger, generation);
    let mut skipped: HashSet<String> = HashSet::new();
    let mut passes = 0;
    loop {
        let listing = match list_entries(&admissions) {
            Ok(listing) => listing.unwrap_or_default(),
            Err(_) => {
                return Ok(ClosureOutcome::Refused(
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
                ));
            }
        };
        #[cfg(test)]
        if passes == 0
            && let Some(hook) = AFTER_FIRST_LISTING.with(|hook| hook.borrow_mut().take())
        {
            hook();
        }
        for name in listing.skipped {
            if skipped.insert(name.clone()) {
                notes.push(format!("admissions/{name} is not a launch and was skipped"));
            }
        }
        if listing.names.is_empty() && obligations.is_empty() {
            break;
        }
        passes += 1;
        if passes > MAX_PASSES {
            return Ok(ClosureOutcome::Refused(
                BootstrapRecoveryReason::CoordinatorNotLive,
            ));
        }
        if let Some(reason) = settle_listing(
            ledger,
            generation,
            &listing.names,
            &mut obligations,
            &mut tally,
            &mut notes,
            authority,
        )? {
            return Ok(ClosureOutcome::Refused(reason));
        }
        #[cfg(test)]
        if fault(Fault::InferEmpty) {
            break;
        }
    }
    close.finish(record, set_aside, notes, tally)?;
    Ok(ClosureOutcome::Closed)
}

struct Close<'a> {
    ledger: &'a ParentLossLedger,
    generation: ParentLossGeneration,
    successor_generation: ParentLossGeneration,
    authority: &'a ClosingAuthority<'a>,
    supervisor: Option<ProcessInstance>,
    coordinator: Option<ProcessInstance>,
    observations: (AuthorityObservation, AuthorityObservation),
}

impl Close<'_> {
    /// Write the terminal, then delete `census-started.json`, then
    /// `census-obligation.json`, so no released reader ever finds a started
    /// census without its obligations.
    fn finish(
        &self,
        record: Option<ParentLossGenerationRecord>,
        set_aside: Vec<String>,
        mut notes: Vec<String>,
        tally: Tally,
    ) -> Result<(), ParentLossLedgerError> {
        let (ledger, generation) = (self.ledger, self.generation);
        // Every terminal keeps a `ledger.json` and digest pair: the one the
        // record already binds when it is intact, else the empty set.
        let ledger_path = ledger.sealed_ledger_path(generation);
        let bound = record
            .as_ref()
            .and_then(|record| record.sealed_ledger_digest.clone())
            .filter(|digest| {
                fs::read(&ledger_path).is_ok_and(|bytes| digest_bytes(&bytes) == *digest)
            });
        let digest = match bound {
            Some(digest) => digest,
            None => {
                if ledger_path.exists() {
                    move_to_discharged(ledger, generation, &ledger_path)?;
                    notes.push("an unbound ledger.json was replaced by the empty set".to_owned());
                }
                ledger.write_sealed_ledger(generation, &Vec::<SealedAdmission>::new())?
            }
        };
        if self.supervisor.is_none() {
            notes.push(
                "no supervisor identity was recorded; the record names the closer".to_owned(),
            );
        }
        if tally.omitted > 0 {
            notes.push(format!(
                "{} further noteworthy admissions are counted in the summary only",
                tally.omitted
            ));
        }
        let mut record = record.unwrap_or_else(|| ParentLossGenerationRecord {
            schema: PARENT_LOSS_LEDGER_SCHEMA_V1,
            generation,
            coordinator: self.coordinator,
            supervisor: self.supervisor.unwrap_or(self.authority.closed_by),
            sealed_ledger_digest: None,
            terminal: None,
            supervisor_heartbeat: None,
            closure: None,
        });
        match &record.terminal {
            Some(ParentLossTerminalDisposition::Unresolved { .. }) => {}
            other => {
                if let Some(other) = other {
                    notes.push(format!("its terminal {other:?} did not validate"));
                }
                record.terminal = Some(ParentLossTerminalDisposition::Unresolved {
                    reason: ParentLossUnresolvedReason::AuthoritiesLost,
                });
            }
        }
        record.sealed_ledger_digest = Some(digest);
        let heartbeat = match (record.supervisor_heartbeat.as_deref(), self.supervisor) {
            (Some(filename), Some(instance)) => {
                clear_supervisor_heartbeat(ledger, filename, instance)
            }
            _ => HeartbeatRetirement::NotRecorded,
        };
        record.closure = Some(AbandonedGenerationClosure {
            schema: PARENT_LOSS_CLOSURE_SCHEMA_V1,
            closed_by: self.authority.closed_by,
            successor_generation: self.successor_generation,
            closed_at: unix_seconds(),
            supervisor: self.observations.0,
            coordinator: self.observations.1,
            heartbeat,
            set_aside,
            notes,
            admissions: tally.rows,
            summary: Some(tally.summary),
        });
        write_json(ledger.record_path(generation), &record, json_options())?;
        stop_at(Stop::TerminalWritten)?;
        remove_if_present(
            &ledger
                .generation_path(generation)
                .join("census-started.json"),
        )?;
        stop_at(Stop::StartedDeleted)?;
        remove_if_present(&ledger.census_obligation_path(generation))?;
        Ok(())
    }
}

fn remove_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        other => other,
    }
}

// ---- the prune ----

/// How much a closed generation's record says, read cheaply.
#[derive(Deserialize)]
pub(crate) struct RecordGlance {
    pub(crate) generation: ParentLossGeneration,
    #[serde(default)]
    pub(crate) terminal: Option<ParentLossTerminalDisposition>,
    #[serde(default)]
    pub(crate) closure: Option<serde::de::IgnoredAny>,
}

impl RecordGlance {
    pub(crate) fn read(path: &Path) -> io::Result<Option<Self>> {
        match fs::read(path) {
            Ok(bytes) => Ok(serde_json::from_slice(&bytes).ok()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    /// Settled for good: a clean terminal, or closed by a successor.
    pub(crate) fn is_settled(&self) -> bool {
        self.closure.is_some()
            || matches!(
                self.terminal,
                Some(
                    ParentLossTerminalDisposition::Completed { .. }
                        | ParentLossTerminalDisposition::RetiredExpected
                        | ParentLossTerminalDisposition::CancelledBeforeAdmission
                )
            )
    }
}

/// Every numeric generation directory, ascending.
pub(crate) fn generation_directories(
    ledger: &ParentLossLedger,
) -> io::Result<Vec<ParentLossGeneration>> {
    let mut found = Vec::new();
    match fs::read_dir(ledger.root_path().join("generations")) {
        Ok(children) => {
            for child in children {
                let child = child?;
                if let Some(generation) = child
                    .file_name()
                    .to_str()
                    .and_then(|name| name.parse::<ParentLossGeneration>().ok())
                    && child.file_type()?.is_dir()
                {
                    found.push(generation);
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    found.sort_unstable();
    Ok(found)
}

/// Delete a tree slowly, a file at a time. Killed at any point it leaves a
/// smaller tree, which the next prune finishes.
fn remove_tree_gently(path: &Path, removed: &mut usize) {
    let Ok(metadata) = fs::symlink_metadata(path) else {
        return;
    };
    if metadata.is_dir() {
        if let Ok(children) = fs::read_dir(path) {
            let children: Vec<PathBuf> = children.flatten().map(|child| child.path()).collect();
            for child in children {
                remove_tree_gently(&child, removed);
            }
        }
        let _ = fs::remove_dir(path);
    } else {
        let _ = fs::remove_file(path);
    }
    *removed += 1;
    if (*removed).is_multiple_of(256) {
        thread::sleep(Duration::from_millis(5));
    }
}

/// Delete what settled generations below the highest no longer need: the
/// trash, the entries left behind, and the released closer's caches. Never the
/// highest generation, never a generation still owed anything, never on the
/// start path.
pub(crate) fn prune_settled_generations(ledger: &ParentLossLedger) {
    let Ok(generations) = generation_directories(ledger) else {
        return;
    };
    let Some(&highest) = generations.last() else {
        return;
    };
    let mut removed = 0usize;
    for generation in generations.into_iter().filter(|g| *g < highest) {
        let settled = RecordGlance::read(&ledger.record_path(generation))
            .ok()
            .flatten()
            .is_some_and(|glance| glance.generation == generation && glance.is_settled());
        if !settled {
            continue;
        }
        for path in [
            discharged_dir(ledger, generation),
            admission_directory(ledger, generation),
            ledger.closure_recovery_dir(generation),
            ledger.closure_recovery_legacy_path(generation),
        ] {
            remove_tree_gently(&path, &mut removed);
        }
    }
}
