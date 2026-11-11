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
    /// macOS `kern.boottime` in seconds, or `None`.
    fn boot_time(&self) -> Option<u64> {
        current_boot_time()
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
    HealDamagedIdentity,
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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BootRecord {
    schema: u32,
    boot: String,
    /// macOS `kern.boottime`, seconds. Absent on Linux and on older records.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    boot_time: Option<u64>,
}

/// A boot session id as a canonical UUID (32 lowercase hex digits). The nil
/// UUID or anything unparseable is no id at all.
fn parse_boot_uuid(text: &str) -> Option<String> {
    let text = text.trim().trim_end_matches('\0').trim();
    let hex: String = text.chars().filter(|c| *c != '-').collect();
    let dashed_ok = !text.contains('-')
        || (text.len() == 36
            && [8, 13, 18, 23]
                .iter()
                .all(|&at| text.as_bytes().get(at) == Some(&b'-')));
    if !dashed_ok || hex.len() != 32 || !hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return None;
    }
    let hex = hex.to_ascii_lowercase();
    (hex.chars().any(|c| c != '0')).then_some(hex)
}

/// The kernel's boot session id: Linux `boot_id`, macOS `kern.bootsessionuuid`.
pub(crate) fn current_boot_id() -> Option<String> {
    #[cfg(target_os = "linux")]
    {
        fs::read_to_string("/proc/sys/kernel/random/boot_id")
            .ok()
            .as_deref()
            .and_then(parse_boot_uuid)
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
            .and_then(parse_boot_uuid)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        None
    }
}

/// macOS `kern.boottime` in seconds; `None` elsewhere or when unreadable.
pub(crate) fn current_boot_time() -> Option<u64> {
    #[cfg(target_os = "macos")]
    {
        let mut value = libc::timeval {
            tv_sec: 0,
            tv_usec: 0,
        };
        let mut length = std::mem::size_of::<libc::timeval>();
        // SAFETY: the name is NUL-terminated and `value` is a writable
        // `timeval` of exactly `length` bytes owned by this frame.
        let status = unsafe {
            libc::sysctlbyname(
                c"kern.boottime".as_ptr(),
                (&mut value as *mut libc::timeval).cast(),
                &mut length,
                std::ptr::null_mut(),
                0,
            )
        };
        (status == 0 && value.tv_sec > 0).then(|| value.tv_sec as u64)
    }
    #[cfg(not(target_os = "macos"))]
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
                boot_time: current_boot_time(),
            },
            json_options(),
        );
    }
}

/// The boot settlement rule. On Linux the session id alone decides. On
/// macOS both the session id and `kern.boottime` must be recorded, read, and
/// differ; a record without the boot time never settles there.
fn boot_settles(
    recorded: &BootRecord,
    current: &str,
    current_time: Option<u64>,
    macos: bool,
) -> bool {
    let Some(recorded_id) = parse_boot_uuid(&recorded.boot) else {
        return false;
    };
    if recorded.schema != BOOT_SCHEMA_V1 || recorded_id == current {
        return false;
    }
    if !macos {
        return true;
    }
    matches!(
        (recorded.boot_time, current_time),
        (Some(recorded), Some(current)) if recorded != current
    )
}

/// Whether the generation provably ran on another boot. Anything that cannot
/// be read settles nothing.
fn boot_changed(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    retirer: &dyn AdmissionRetirer,
    supervisor: Option<ProcessInstance>,
) -> bool {
    let Some(current) = retirer.boot_id().as_deref().and_then(parse_boot_uuid) else {
        return false;
    };
    let recorded = fs::read(boot_path(ledger, generation))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<BootRecord>(&bytes).ok());
    if recorded.is_some_and(|recorded| {
        boot_settles(
            &recorded,
            &current,
            retirer.boot_time(),
            cfg!(target_os = "macos"),
        )
    }) {
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

    /// No identity was read, yet a drop that would carry one is there and
    /// unreadable: a destroyed sole identity, which never heals into absence.
    pub(crate) fn identity_destroyed(&self) -> bool {
        self.identities.is_empty()
            && self.unreadable.iter().any(|name| {
                matches!(
                    *name,
                    "result.json" | "acknowledgement.json" | "spawn-identity.json"
                )
            })
    }

    /// Whether the entry needs no process observed: it never had one.
    pub(crate) fn settled_without_process(&self) -> bool {
        self.identities.is_empty() && self.launcher_reaped()
    }

    /// The launcher recorded that no process outlived the launch.
    pub(crate) fn launcher_reaped(&self) -> bool {
        matches!(
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
        // A drop that is there and cannot be read (a directory where it
        // belongs, an I/O error) is unreadable, the same as a torn one.
        Err(_) => {
            unreadable.push(name);
            return Ok(None);
        }
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

/// A state directory found as a regular file is set aside under a suffixed
/// name, so it can never refuse a start. `true` when one was set aside.
fn set_aside_if_file(path: &Path) -> io::Result<bool> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if !metadata.is_dir() => {
            let name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .unwrap_or_default();
            fs::rename(
                path,
                path.with_file_name(format!("{name}.not-a-directory.{}", unique_suffix())),
            )?;
            Ok(true)
        }
        _ => Ok(false),
    }
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

/// Move an entry out of the outstanding set as the sealing authority, through
/// the one discharge choke point. A rename, no fsync: a crash only re-observes
/// it, and dead stays dead because identity is exact. An entry already gone is
/// already discharged; any other kept outcome is an error, so the caller never
/// treats the entry as settled while it is still listed.
pub(crate) fn discharge_sealed(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
    launch_id: &str,
) -> Result<(), ParentLossLedgerError> {
    use super::parent_loss_admission::{DischargeKept, Discharged, Discharger, discharge};
    match discharge(ledger, generation, launch_id, Discharger::Sealing) {
        Ok(Discharged::Moved | Discharged::Kept(DischargeKept::EntryMissing)) => Ok(()),
        Ok(Discharged::Kept(kept)) => Err(ParentLossLedgerError::Io(io::Error::other(format!(
            "admission {launch_id} was not discharged: {kept:?}"
        )))),
        Err(error) => Err(ParentLossLedgerError::Io(io::Error::other(
            error.to_string(),
        ))),
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
    set_aside_if_file(&trash)?;
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
    /// `census-started.json`, or a legacy refusal marker: retirement began.
    began: bool,
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
    let began = started || !markers.is_empty();
    Ok(CensusEvidence {
        main,
        main_damaged,
        copies,
        markers,
        began,
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
    authority: &ClosingAuthority<'_>,
) -> Result<Option<Obligations>, ParentLossLedgerError> {
    let generation_dir = ledger.generation_path(generation);
    let evidence = census_evidence(&generation_dir)?;
    if evidence.lost {
        return Ok(None);
    }
    let authoritative_census = evidence.main.is_some() || !evidence.copies.is_empty();
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
    let admissions = admission_directory(ledger, generation);
    let authoritative: HashSet<String> = merged.keys().cloned().collect();
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
            let live = match read_json_durable_validated::<LiveRecoveryFile>(
                ArtifactId::ParentLossClosureRecovery,
                &path,
                |_| Ok(()),
            ) {
                Ok(DurableRead::Present(live)) => live,
                Ok(DurableRead::Absent) => continue,
                _ => {
                    // Without an authoritative census, a damaged cache after
                    // retirement began is lost identities: that refuses.
                    if evidence.began && !authoritative_census {
                        return Ok(None);
                    }
                    notes.push(format!("the recovery cache for {launch_id} was damaged"));
                    continue;
                }
            };
            // A cache the census does not hold is kept when its own drops or
            // this journal's environment name its root, or while anything it
            // names is not proven gone. Only a cache whose every process is
            // observed gone, and that nothing authorizes, is dropped.
            if !authoritative.contains(&launch_id) {
                let by_drops = read_entry(&admissions, &launch_id).is_ok_and(|entry| {
                    entry
                        .identities
                        .iter()
                        .any(|identity| identity.instance == live.root && identity.uid == live.uid)
                });
                let by_environment = live.sources.as_ref().is_some_and(|sources| {
                    sources
                        .environment_identity
                        .as_ref()
                        .is_some_and(|identity| identity.instance == live.root)
                        && sources.environment_journal.as_deref()
                            == ledger.canonical_root().to_str()
                });
                let open = std::iter::once((live.root, live.uid))
                    .chain(live.descendants.iter().map(|d| (d.instance, d.uid)))
                    .any(|(instance, uid)| {
                        !matches!(
                            settle(authority.source, authority.retirer, instance, uid),
                            Settlement::Gone(_)
                        )
                    });
                if !by_drops && !by_environment && !open {
                    notes.push(format!(
                        "a recovery cache for {launch_id} named only processes observed gone; it was discarded"
                    ));
                    continue;
                }
            }
            merge_obligation(&mut merged, launch_id, live);
        }
    }
    let recovery = ledger.closure_recovery_dir(generation);
    let admissions = admission_directory(ledger, generation);
    let authoritative: HashSet<String> = merged.keys().cloned().collect();
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
            let Ok(DurableRead::Present(live)) = read_json_durable_validated::<LiveRecoveryFile>(
                ArtifactId::ParentLossClosureRecovery,
                &path,
                |_| Ok(()),
            ) else {
                continue;
            };
            // A cache the census does not hold names a root only its own
            // launch's drops can authorize; anything else is never signalled.
            if !authoritative.contains(&launch_id)
                && !read_entry(&admissions, &launch_id).is_ok_and(|entry| {
                    entry
                        .identities
                        .iter()
                        .any(|identity| identity.instance == live.root && identity.uid == live.uid)
                })
            {
                notes.push(format!(
                    "a recovery cache for {launch_id} named a process its admission does not; it was discarded"
                ));
                continue;
            }
            merge_obligation(&mut merged, launch_id, live);
        }
    }
    // Every obligation carries the drops its root came from, so a released
    // binary can still authorize it.
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

/// Every descendant of one live identity. `Err` when the tree cannot be
/// read completely; an identity gone since it was observed has none.
#[cfg(any(target_os = "linux", target_os = "macos"))]
fn snapshot_descendants(
    root: ProcessInstance,
    uid: u32,
    source: &dyn ProcessInstanceSource,
) -> Result<Vec<LiveDescendantIdentity>, ()> {
    let snapshot = match exact_descendant_tree(root, uid, source) {
        Ok(snapshot) => snapshot,
        Err(crate::process::DescendantObservationFailure::RootNotSameOrExited) => {
            return Ok(Vec::new());
        }
        Err(_) => return Err(()),
    };
    Ok(snapshot
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
        .collect())
}

#[cfg(not(any(target_os = "linux", target_os = "macos")))]
fn snapshot_descendants(
    _root: ProcessInstance,
    _uid: u32,
    _source: &dyn ProcessInstanceSource,
) -> Result<Vec<LiveDescendantIdentity>, ()> {
    Ok(Vec::new())
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
    // Every live identity's subtree is read, root or descendant, from the
    // observation that found it live. An identity inside a subtree already
    // read is covered by it.
    let mut covered: Vec<(String, ProcessInstance)> = Vec::new();
    for (launch_id, live) in obligations.iter_mut() {
        let mut identities = vec![(live.root, live.uid)];
        identities.extend(live.descendants.iter().map(|d| (d.instance, d.uid)));
        for (instance, uid) in identities {
            if covered.contains(&(launch_id.clone(), instance))
                || settle(source, retirer, instance, uid) != Settlement::Live
            {
                continue;
            }
            match snapshot_descendants(instance, uid, source) {
                Ok(found) => {
                    covered.push((launch_id.clone(), instance));
                    for descendant in found {
                        covered.push((launch_id.clone(), descendant.instance));
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
                Err(()) => {
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
    // ⛔ A live identity whose subtree was not read this start is never
    // signalled: its obligation waits for the next start.
    for target in &targets {
        let (instance, _) = target_identity(&obligations[&target.0], target.1);
        if state[target] == Settlement::Live && !covered.contains(&(target.0.clone(), instance)) {
            blind.insert(target.0.clone());
        }
    }
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
    // Only what was live, read and ours at this observation is ever signalled.
    let signalable: HashSet<Target> = targets
        .iter()
        .filter(|target| {
            state[*target] == Settlement::Live
                && !blind.contains(&target.0)
                && !foreign.contains(&target.0)
        })
        .cloned()
        .collect();

    let now_ms = unix_millis();
    let mut any_live = false;
    for target in &signalable {
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
        if signalable.contains(target) {
            let (instance, uid) = target_identity(&obligations[&target.0], target.1);
            if retirer.owner(instance.pid) == ProcessOwner::Uid(uid) {
                let _ = retirer.signal(instance, SignalKind::Terminate);
            }
        }
    }
    stop_at(Stop::TermSent)?;

    // Each start sends KILL once to every identity past its escalation, so a
    // KILL that never landed is sent again by the next start.
    let mut killed: HashSet<Target> = HashSet::new();
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
            if observed != Settlement::Live || !signalable.contains(target) {
                continue;
            }
            let t = timing(&obligations[&target.0], target.1);
            if !killed.contains(target) {
                if now_ms >= t.escalate_at {
                    due.push(target.clone());
                }
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
            let mut sent = Vec::new();
            for target in &due {
                killed.insert(target.clone());
                let (instance, uid) = target_identity(&obligations[&target.0], target.1);
                if matches!(source.observe(&instance), InstanceVerdict::SameLive { .. })
                    && retirer.owner(instance.pid) == ProcessOwner::Uid(uid)
                    && retirer.signal(instance, SignalKind::Kill).is_ok()
                {
                    sent.push(target.clone());
                }
            }
            // The reap grace starts once a KILL is sent, and a stored
            // deadline only ever moves earlier.
            let reap = unix_millis() + REAP_GRACE_MS;
            for target in &sent {
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

/// Seal the pointer, then drain the admission lock: launchers check the phase
/// under that lock, so once it is taken no launch stands ahead of the seal.
/// `false` when a live launcher still holds it past the wait.
fn seal_pointer(
    ledger: &ParentLossLedger,
    generation: ParentLossGeneration,
) -> Result<bool, ParentLossLedgerError> {
    #[cfg(test)]
    if fault(Fault::SkipSeal) {
        return Ok(true);
    }
    ledger.mark_sealed_for_closure(generation)?;
    Ok(admission_lock(ledger, generation, ADMISSION_SEAL_LOCK_TIMEOUT).is_ok())
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
    let mut out_of_time = false;
    for (index, window) in names.chunks(READ_WINDOW).enumerate() {
        // Every start settles at least one window, so even a start whose
        // budget is gone strictly shrinks the set.
        if index > 0 && Instant::now() >= authority.deadline {
            out_of_time = true;
            break;
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
            let destroyed = entry.identity_destroyed();
            #[cfg(test)]
            let destroyed = destroyed && !fault(Fault::HealDamagedIdentity);
            if destroyed {
                notes.push(format!(
                    "admission {}: its only identity drop is unreadable, so it is kept until the \
                     computer restarts",
                    entry.launch_id
                ));
            }
            if blind || destroyed {
                unverifiable += 1;
                continue;
            }
            if obligations.contains_key(&entry.launch_id) {
                held.insert(entry.launch_id.clone(), entry);
                continue;
            }
            // A launcher that confirmed its own reap keeps its finding, even
            // beside an acknowledgement the child left behind.
            let finding = if entry.identities.is_empty() || entry.launcher_reaped() {
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
    if out_of_time {
        return Ok(Some(BootstrapRecoveryReason::CoordinatorNotLive));
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
    if boot_changed(ledger, generation, authority.retirer, supervisor) {
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
    let Some(mut obligations) = fold_census(ledger, generation, &mut notes, authority)? else {
        return Ok(ClosureOutcome::Refused(
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
        ));
    };
    let admissions = admission_directory(ledger, generation);
    if set_aside_if_file(&admissions)? {
        notes.push(
            "admissions was a file, not a directory; it was set aside and read as empty".to_owned(),
        );
    }
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
    use crate::lifecycle::parent_loss_ledger::{CoordinatorLease, read_json_optional};
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
        table_births: HashMap<u32, TableBirth>,
        /// Children of a pid, as the census reports them.
        children: HashMap<u32, Vec<u32>>,
        /// The next this-many inspects of a pid read `Unverifiable`.
        blind_reads: HashMap<u32, usize>,
    }

    #[derive(Clone, Default)]
    struct FakeSource(Arc<Mutex<FakeTable>>);

    impl ProcessInstanceSource for FakeSource {
        fn inspect(&self, pid: u32) -> InspectResult {
            let mut table = self.0.lock().expect("fake table");
            if let Some(left) = table.blind_reads.get_mut(&pid)
                && *left > 0
            {
                *left -= 1;
                return InspectResult::Unverifiable;
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
            let parent_of: HashMap<u32, u32> = table
                .children
                .iter()
                .flat_map(|(parent, kids)| kids.iter().map(move |kid| (*kid, *parent)))
                .collect();
            let rows = table
                .live
                .values()
                .map(|(instance, uid)| CensusRow {
                    instance: *instance,
                    uid: *uid,
                    ppid: parent_of.get(&instance.pid).copied().unwrap_or(1),
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
        boot: Option<Option<String>>,
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

        fn table_birth(&self, pid: u32) -> TableBirth {
            let table = self.table.lock().expect("fake table");
            table
                .table_births
                .get(&pid)
                .copied()
                .unwrap_or(TableBirth::Unknown)
        }

        fn boot_id(&self) -> Option<String> {
            match &self.boot {
                Some(boot) => boot.clone(),
                None => current_boot_id(),
            }
        }
    }

    impl FakeRetirer {
        fn signals(&self) -> Vec<(u32, &'static str)> {
            let mut signals = self.signals.lock().expect("signals").clone();
            signals.sort_unstable();
            signals
        }
    }

    fn fake(
        live: &[(ProcessInstance, u32)],
        obeys_terminate: &[u32],
        immortal: &[u32],
    ) -> (FakeSource, FakeRetirer) {
        let table = Arc::new(Mutex::new(FakeTable {
            live: live.iter().map(|(i, uid)| (i.pid, (*i, *uid))).collect(),
            ..FakeTable::default()
        }));
        let retirer = FakeRetirer {
            table: Arc::clone(&table),
            signals: Mutex::new(Vec::new()),
            obeys_terminate: obeys_terminate.iter().copied().collect(),
            immortal: immortal.iter().copied().collect(),
            boot: None,
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

    fn identity(
        generation: ParentLossGeneration,
        launch_id: &str,
        instance: ProcessInstance,
    ) -> AdmissionIdentity {
        AdmissionIdentity {
            generation,
            launch_id: launch_id.to_owned(),
            instance,
            uid: 501,
            parent_launch_id: None,
        }
    }

    fn admit(
        journal: &Path,
        generation: ParentLossGeneration,
        launch_id: &str,
        service: Option<HostedServiceKind>,
        child: Option<ProcessInstance>,
    ) {
        write_parent_loss_admission_intent(
            journal,
            &AdmissionIntent::new(generation, launch_id, service, None),
        )
        .expect("intent");
        if let Some(child) = child {
            let id = identity(generation, launch_id, child);
            acknowledge_parent_loss_admission(journal, id.clone()).expect("ack");
            write_parent_loss_admission_result(
                journal,
                generation,
                launch_id,
                &AdmissionResult {
                    schema: 1,
                    identity: Some(id),
                    state: AdmissionResultState::Admitted,
                },
            )
            .expect("result");
        }
    }

    fn acquire_lease(ledger: &ParentLossLedger) -> CoordinatorLease {
        ledger
            .acquire_coordinator_lease()
            .expect("lease acquisition")
            .expect("lease free")
    }

    /// Bounds a hang only; tests about the deadline pass their own.
    const AMPLE_BUDGET: Duration = Duration::from_secs(60);

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

    fn start(
        ledger: &ParentLossLedger,
        lease: &CoordinatorLease,
        source: &dyn ProcessInstanceSource,
        retirer: &dyn AdmissionRetirer,
        budget: Duration,
    ) -> Result<ActiveGeneration, ParentLossLedgerError> {
        ledger.reserve_generation_closing_abandoned(
            instance(11, 3),
            [],
            lease,
            &authority(source, retirer, budget),
        )
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

    fn summary_of(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> ClosureSummary {
        closure_of(ledger, generation).summary.expect("summary")
    }

    fn finding(closure: &AbandonedGenerationClosure, launch_id: &str) -> Option<AdmissionFinding> {
        closure
            .admissions
            .iter()
            .find(|admission| admission.launch_id == launch_id)
            .map(|admission| admission.finding)
    }

    fn census(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> Obligations {
        read_json_optional::<CensusObligationFile>(&ledger.census_obligation_path(generation))
            .expect("census readable")
            .map(|file| file.obligations)
            .unwrap_or_default()
    }

    fn entries(ledger: &ParentLossLedger, generation: ParentLossGeneration) -> Vec<String> {
        list_entries(&admission_directory(ledger, generation))
            .expect("listing")
            .map(|listing| listing.names)
            .unwrap_or_default()
    }

    fn refused(
        result: &Result<ActiveGeneration, ParentLossLedgerError>,
        reason: BootstrapRecoveryReason,
    ) -> bool {
        matches!(result, Err(ParentLossLedgerError::RecoveryRequired(r)) if *r == reason)
    }

    fn with_stop<T>(point: Stop, run: impl FnOnce() -> T) -> T {
        STOP.with(|stop| stop.set(Some(point)));
        let out = run();
        STOP.with(|stop| stop.set(None));
        out
    }

    fn with_fault<T>(which: Fault, run: impl FnOnce() -> T) -> T {
        FAULT.with(|fault| fault.set(Some(which)));
        let out = run();
        FAULT.with(|fault| fault.set(None));
        out
    }

    /// A persisted retirement obligation, as either binary writes it.
    fn obligation(
        generation: ParentLossGeneration,
        launch_id: &str,
        root: ProcessInstance,
        descendants: Vec<LiveDescendantIdentity>,
        term_requested: bool,
    ) -> LiveRecoveryFile {
        LiveRecoveryFile {
            sources: None,
            schema: RECOVERY_SCHEMA_V1,
            kind: RecoveryArtifactKind::Live,
            generation,
            sealed_phase: ParentLossPhase::Sealed,
            launch_id: launch_id.to_owned(),
            service: None,
            root,
            uid: 501,
            parent_launch_id: None,
            binding_digest: compute_live_binding_digest(root, 501, &descendants).unwrap(),
            descendants,
            escalate_at_unix_ms: if term_requested {
                unix_millis() + TERM_GRACE_MS
            } else {
                0
            },
            reap_deadline_unix_ms: None,
            term_requested,
            escalated: false,
        }
    }

    fn descendant(
        child: ProcessInstance,
        uid: u32,
        term_requested: bool,
    ) -> LiveDescendantIdentity {
        LiveDescendantIdentity {
            instance: child,
            uid,
            ppid: 0,
            pgid: 0,
            escalate_at_unix_ms: if term_requested {
                unix_millis() + TERM_GRACE_MS
            } else {
                0
            },
            reap_deadline_unix_ms: None,
            term_requested,
            escalated: false,
        }
    }

    fn write_census(
        ledger: &ParentLossLedger,
        generation: ParentLossGeneration,
        obligations: &[LiveRecoveryFile],
    ) {
        let map: Obligations = obligations
            .iter()
            .map(|live| (live.launch_id.clone(), live.clone()))
            .collect();
        persist_census(ledger, generation, &map).expect("census");
    }

    // ---- the 09-16 safety line, ported ----

    #[test]
    fn an_open_generation_whose_authorities_are_gone_is_closed_and_the_successor_boots() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "sense-a",
            Some(HostedServiceKind::Sense),
            Some(instance(4242, 42)),
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);

        let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET)
            .expect("an abandoned generation must not brick the journal");

        assert_eq!(successor.generation, active.generation + 1);
        let record = ledger.record(active.generation).unwrap().unwrap();
        assert_eq!(
            record.terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AuthoritiesLost,
            })
        );
        let closure = record.closure.expect("closure");
        assert_eq!(closure.successor_generation, successor.generation);
        assert_eq!(closure.supervisor, AuthorityObservation::Exited);
        assert_eq!(closure.coordinator, AuthorityObservation::Exited);
        assert_eq!(closure.summary.as_ref().map(|s| s.exited), Some(1));
        assert!(
            closure.admissions.is_empty(),
            "an exited row is a count only"
        );
        assert!(retirer.signals().is_empty());
        let sealed = fs::read(ledger.sealed_ledger_path(active.generation)).expect("ledger");
        assert_eq!(record.sealed_ledger_digest, Some(digest_bytes(&sealed)));
        assert!(entries(&ledger, active.generation).is_empty());
    }

    fn a_live_peer_refuses(peer_is_supervisor: bool) -> Result<(), String> {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let live_child = instance(4343, 43);
        admit(
            directory.path(),
            active.generation,
            "child",
            None,
            Some(live_child),
        );
        let peer = if peer_is_supervisor {
            active.supervisor
        } else {
            active.coordinator.expect("coordinator")
        };
        let (source, retirer) = fake(&[(peer, 501), (live_child, 501)], &[live_child.pid], &[]);
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        let expected = if peer_is_supervisor {
            BootstrapRecoveryReason::SupervisorLive
        } else {
            BootstrapRecoveryReason::ActiveCoordinator
        };
        if !refused(&result, expected) {
            return Err(format!("a live peer did not refuse: {result:?}"));
        }
        if !retirer.signals().is_empty() {
            return Err("a live peer's admission was signalled".to_owned());
        }
        if ledger
            .record(active.generation)
            .unwrap()
            .unwrap()
            .closure
            .is_some()
        {
            return Err("the generation was closed under a live peer".to_owned());
        }
        Ok(())
    }

    #[test]
    fn a_live_coordinator_still_refuses_the_closer() {
        a_live_peer_refuses(false).unwrap();
    }

    #[test]
    fn a_live_supervisor_still_refuses_the_closer() {
        a_live_peer_refuses(true).unwrap();
    }

    #[test]
    fn skipping_the_veto_turns_the_live_peer_tests_red() {
        with_fault(Fault::SkipVeto, || {
            assert!(a_live_peer_refuses(true).is_err());
            assert!(a_live_peer_refuses(false).is_err());
        });
    }

    #[test]
    fn an_admission_that_outlives_the_deadline_refuses_this_start_and_leaves_the_generation_open() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let stubborn = instance(5000, 50);
        admit(
            directory.path(),
            active.generation,
            "stubborn",
            None,
            Some(stubborn),
        );
        let (source, retirer) = fake(&[(stubborn, 501)], &[], &[stubborn.pid]);
        let lease = acquire_lease(&ledger);

        let result = start(
            &ledger,
            &lease,
            &source,
            &retirer,
            Duration::from_millis(50),
        );

        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionLive
        ));
        assert_eq!(
            ledger.active_generation().unwrap().unwrap().generation,
            active.generation
        );
        assert!(
            ledger
                .record(active.generation)
                .unwrap()
                .unwrap()
                .closure
                .is_none()
        );
        assert_eq!(entries(&ledger, active.generation), vec!["stubborn"]);
        assert!(census(&ledger, active.generation).contains_key("stubborn"));
    }

    #[test]
    fn a_child_that_obeys_sigterm_is_not_escalated_and_the_signals_are_recorded() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let obedient = instance(6000, 60);
        admit(
            directory.path(),
            active.generation,
            "obedient",
            None,
            Some(obedient),
        );
        let (source, retirer) = fake(&[(obedient, 501)], &[obedient.pid], &[]);
        let lease = acquire_lease(&ledger);

        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");

        assert_eq!(retirer.signals(), vec![(obedient.pid, "term")]);
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(
            finding(&closure, "obedient"),
            Some(AdmissionFinding::Retired { escalated: false })
        );
        assert!(!ledger.census_obligation_path(active.generation).exists());
        assert!(
            !ledger
                .generation_path(active.generation)
                .join("census-started.json")
                .exists()
        );
    }

    #[test]
    fn retirement_is_concurrent_against_one_deadline() {
        // Five stubborn roots: TERM goes to all at once, KILL to each at its
        // own escalation, all inside one deadline, not one grace per root.
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let roots: Vec<_> = (0..5).map(|i| instance(7100 + i, 71)).collect();
        for (i, root) in roots.iter().enumerate() {
            admit(
                directory.path(),
                active.generation,
                &format!("r{i}"),
                None,
                Some(*root),
            );
        }
        let mut map = Obligations::new();
        for (i, root) in roots.iter().enumerate() {
            let mut live = obligation(active.generation, &format!("r{i}"), *root, Vec::new(), true);
            live.escalate_at_unix_ms = unix_millis() + 200;
            map.insert(live.launch_id.clone(), live);
        }
        persist_census(&ledger, active.generation, &map).unwrap();
        let live: Vec<_> = roots.iter().map(|r| (*r, 501)).collect();
        let (source, retirer) = fake(&live, &[], &[]);
        let lease = acquire_lease(&ledger);
        let began = Instant::now();
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");
        assert!(
            began.elapsed() < Duration::from_secs(3),
            "five escalations took {:?}",
            began.elapsed()
        );
        let signals = retirer.signals.lock().unwrap().clone();
        let first_kill = signals
            .iter()
            .position(|(_, s)| *s == "kill")
            .expect("kills");
        assert!(signals[..first_kill].iter().all(|(_, s)| *s == "term"));
        assert_eq!(first_kill, 5, "every TERM is sent before any KILL");
        assert_eq!(summary_of(&ledger, active.generation).escalated, 5);
    }

    #[test]
    fn incomplete_descendant_coverage_refuses_without_retiring_the_root() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let parent = instance(7000, 70);
        admit(
            directory.path(),
            active.generation,
            "parent",
            None,
            Some(parent),
        );
        let (source, retirer) = fake(&[(parent, 501)], &[parent.pid], &[]);
        source.0.lock().unwrap().incomplete_census = true;
        let lease = acquire_lease(&ledger);

        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);

        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
        ));
        assert!(retirer.signals().is_empty());
    }

    #[test]
    fn an_uninspectable_pid_is_settled_by_its_owner_or_refuses() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let (gone, foreign, mine) = (instance(8001, 80), instance(8002, 80), instance(8003, 80));
        for (id, child) in [("gone", gone), ("foreign", foreign), ("mine", mine)] {
            admit(directory.path(), active.generation, id, None, Some(child));
        }
        let (source, retirer) = fake(&[], &[], &[]);
        {
            let mut table = source.0.lock().unwrap();
            table.unverifiable.extend([gone.pid, foreign.pid, mine.pid]);
            table.owners.insert(gone.pid, ProcessOwner::Absent);
            table.owners.insert(foreign.pid, ProcessOwner::Uid(9999));
            table.owners.insert(mine.pid, ProcessOwner::Uid(501));
        }
        let lease = acquire_lease(&ledger);

        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);

        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
        ));
        // The two settled ones left the outstanding set; "can't tell" stayed.
        assert_eq!(entries(&ledger, active.generation), vec!["mine"]);
    }

    fn close_one_unreadable_admission_of_this_user(
        owner: ProcessOwner,
        table: TableBirth,
    ) -> (
        TempDir,
        Result<ActiveGeneration, ParentLossLedgerError>,
        usize,
    ) {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let mine = instance(8101, 81);
        admit(
            directory.path(),
            active.generation,
            "mine",
            None,
            Some(mine),
        );
        let (source, retirer) = fake(&[], &[], &[]);
        {
            let mut fake = source.0.lock().unwrap();
            fake.unverifiable.insert(mine.pid);
            fake.owners.insert(mine.pid, owner);
            fake.table_births.insert(mine.pid, table);
        }
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        let signals = retirer.signals().len();
        (directory, result, signals)
    }

    #[test]
    fn an_unreadable_pid_of_this_user_holding_another_birth_is_settled_as_exited() {
        let (directory, result, signals) = close_one_unreadable_admission_of_this_user(
            ProcessOwner::Uid(501),
            TableBirth::Live(ProcessBirth::linux(99, 1, 100)),
        );
        let successor = result.expect("a different birth settles the pid");
        let ledger = ParentLossLedger::open(directory.path()).unwrap();
        assert_eq!(summary_of(&ledger, successor.generation - 1).exited, 1);
        assert_eq!(signals, 0);
    }

    #[test]
    fn an_unreadable_live_admission_of_this_user_or_an_unreadable_owner_keeps_the_generation_pending()
     {
        // The safety line: what the closer cannot prove gone is never adopted,
        // abandoned or closed, and is never signalled blind.
        for (owner, table) in [
            (
                ProcessOwner::Uid(501),
                TableBirth::Live(instance(8101, 81).birth),
            ),
            (ProcessOwner::Uid(501), TableBirth::Unknown),
            (ProcessOwner::Unknown, TableBirth::Absent),
        ] {
            let (directory, result, signals) =
                close_one_unreadable_admission_of_this_user(owner, table);
            assert!(
                refused(
                    &result,
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
                ),
                "{owner:?} {table:?} must refuse, got {result:?}"
            );
            assert_eq!(signals, 0, "{owner:?} {table:?} was signalled");
            let ledger = ParentLossLedger::open(directory.path()).unwrap();
            let pointer = ledger.active_generation().unwrap().unwrap();
            assert_eq!(pointer.generation, 1, "{owner:?} {table:?} advanced");
            assert!(ledger.record(1).unwrap().unwrap().closure.is_none());
            assert_eq!(
                entries(&ledger, 1),
                vec!["mine"],
                "{owner:?} {table:?} discharged"
            );
        }
    }

    fn the_pointer_is_sealed_before_any_launch_can_admit() -> Result<(), String> {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "a",
            None,
            Some(instance(4242, 42)),
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        // Stop once the terminal is written, as a closer killed there.
        let _ = with_stop(Stop::TerminalWritten, || {
            start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET)
        });
        let phase = ledger.active_generation().unwrap().unwrap().phase;
        if phase == ParentLossPhase::Sealed {
            Ok(())
        } else {
            Err(format!("the pointer was left {phase:?}"))
        }
    }

    #[test]
    fn the_pointer_is_sealed_before_the_admissions_are_settled() {
        the_pointer_is_sealed_before_any_launch_can_admit().unwrap();
    }

    #[test]
    fn unfreezing_the_seal_turns_the_seal_test_red() {
        with_fault(Fault::SkipSeal, || {
            assert!(the_pointer_is_sealed_before_any_launch_can_admit().is_err());
        });
    }

    #[test]
    fn an_intent_without_acknowledgement_is_recorded_never_acknowledged() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "unacknowledged",
            Some(HostedServiceKind::Sense),
            None,
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(
            finding(&closure, "unacknowledged"),
            Some(AdmissionFinding::NeverAcknowledged)
        );
    }

    #[test]
    fn a_drop_torn_by_the_kill_closes_and_retires_the_live_child() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let g = active.generation;
        let torn = |launch_id: &str, drop: &str| {
            let path = admission_directory(&ledger, g).join(launch_id).join(drop);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::File::create(path).unwrap();
        };
        let intent = |launch_id: &str| {
            write_parent_loss_admission_intent(
                directory.path(),
                &AdmissionIntent::new(g, launch_id, None, None),
            )
            .unwrap();
        };
        let spawn_torn = instance(4301, 43);
        intent("spawn-torn");
        torn("spawn-torn", "spawn-identity.json");
        acknowledge_parent_loss_admission(directory.path(), identity(g, "spawn-torn", spawn_torn))
            .unwrap();
        let ack_torn = instance(4302, 43);
        intent("ack-torn");
        write_parent_loss_admission_spawn_identity(
            directory.path(),
            &identity(g, "ack-torn", ack_torn),
        )
        .unwrap();
        torn("ack-torn", "acknowledgement.json");
        let result_torn = instance(4303, 43);
        intent("result-torn");
        write_parent_loss_admission_spawn_identity(
            directory.path(),
            &identity(g, "result-torn", result_torn),
        )
        .unwrap();
        torn("result-torn", "result.json");
        torn("intent-torn", "intent.json");
        let (source, retirer) = fake(&[(spawn_torn, 501), (ack_torn, 501)], &[4301, 4302], &[]);
        let lease = acquire_lease(&ledger);

        let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET)
            .expect("a drop torn by the kill must not brick the journal");

        assert_eq!(successor.generation, g + 1);
        assert_eq!(
            retirer.signals(),
            vec![(spawn_torn.pid, "term"), (ack_torn.pid, "term")]
        );
        let closure = closure_of(&ledger, g);
        assert!(matches!(
            finding(&closure, "spawn-torn"),
            Some(AdmissionFinding::Retired { .. })
        ));
        assert!(matches!(
            finding(&closure, "ack-torn"),
            Some(AdmissionFinding::Retired { .. })
        ));
        let summary = closure.summary.unwrap();
        assert_eq!(summary.exited, 1, "result-torn exited");
        assert_eq!(
            summary.never_acknowledged, 1,
            "intent-torn never had a child"
        );
    }

    #[test]
    fn a_missing_record_is_created_and_closed() {
        let directory = TempDir::new().unwrap();
        let ledger = ParentLossLedger::open(directory.path()).unwrap();
        let active = ledger.reserve_generation(instance(10, 1), []).unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert_eq!(successor.generation, active.generation + 1);
        let closure = closure_of(&ledger, active.generation);
        assert_eq!(closure.coordinator, AuthorityObservation::NeverRecorded);
        assert_eq!(closure.supervisor, AuthorityObservation::Exited);
    }

    #[test]
    fn a_malformed_record_is_set_aside_and_closed() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        fs::write(ledger.record_path(active.generation), b"not valid json").unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert!(!closure_of(&ledger, active.generation).set_aside.is_empty());
    }

    #[test]
    fn a_set_aside_pointer_closes_the_open_generation_on_disk() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let live = instance(9191, 91);
        admit(
            directory.path(),
            active.generation,
            "cortex-live",
            None,
            Some(live),
        );
        fs::write(ledger.active_path(), b"{ this is not json").unwrap();
        let (source, retirer) = fake(&[(live, 501)], &[live.pid], &[]);
        let lease = acquire_lease(&ledger);
        let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert!(successor.generation > active.generation);
        assert_eq!(
            finding(&closure_of(&ledger, active.generation), "cortex-live"),
            Some(AdmissionFinding::Retired { escalated: false })
        );
    }

    #[test]
    fn the_dead_supervisors_heartbeat_is_retired_with_the_generation() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let writer_id = WriterId::parse("0123456789abcdef0123456789abcdef").unwrap();
        let run_id = RunId::parse("fedcba9876543210fedcba9876543210").unwrap();
        let filename = v2_heartbeat_filename(&writer_id, &run_id);
        let sync_dir = directory.path().join("health").join("sync");
        fs::create_dir_all(&sync_dir).unwrap();
        let heartbeat_path = sync_dir.join(&filename);
        let supervisor = instance(10, 1);
        let mut record = ledger.record(active.generation).unwrap().unwrap();
        record.supervisor_heartbeat = Some(filename);
        write_json(
            ledger.record_path(active.generation),
            &record,
            json_options(),
        )
        .unwrap();
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
        write_json(&heartbeat_path, &heartbeat, json_options()).unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert_eq!(
            closure_of(&ledger, active.generation).heartbeat,
            HeartbeatRetirement::Removed
        );
        assert!(!heartbeat_path.exists());
    }

    #[test]
    fn a_record_without_a_heartbeat_name_records_that_nothing_was_retired() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert_eq!(
            closure_of(&ledger, active.generation).heartbeat,
            HeartbeatRetirement::NotRecorded
        );
    }

    #[test]
    fn a_sealed_unresolved_generation_is_closed_keeping_its_bound_ledger() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let coordinator = active.coordinator.unwrap();
        admit(
            directory.path(),
            active.generation,
            "sense-a",
            None,
            Some(instance(4242, 42)),
        );
        admit(
            directory.path(),
            active.generation,
            "task-worker-a",
            None,
            None,
        );
        ledger.seal(active.generation, coordinator).unwrap();
        let rows = vec![SealedAdmission {
            launch_id: "sense-a".to_owned(),
            service: None,
            identity: identity(active.generation, "sense-a", instance(4242, 42)),
        }];
        let digest = ledger
            .write_sealed_ledger(active.generation, &rows)
            .unwrap();
        ledger
            .write_terminal_with_digest(
                active.generation,
                coordinator,
                ParentLossTerminalDisposition::Unresolved {
                    reason: ParentLossUnresolvedReason::AdmissionUnconfirmed,
                },
                Some(digest.clone()),
            )
            .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert_eq!(successor.generation, active.generation + 1);
        let record = ledger.record(active.generation).unwrap().unwrap();
        assert_eq!(
            record.terminal,
            Some(ParentLossTerminalDisposition::Unresolved {
                reason: ParentLossUnresolvedReason::AdmissionUnconfirmed,
            }),
            "the coordinator's own terminal is kept"
        );
        assert_eq!(
            record.sealed_ledger_digest,
            Some(digest),
            "the bound pair is kept"
        );
        let summary = record.closure.unwrap().summary.unwrap();
        assert_eq!((summary.exited, summary.never_acknowledged), (1, 1));
    }

    #[test]
    fn a_completed_predecessor_is_not_touched_by_the_closer() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let coordinator = active.coordinator.unwrap();
        ledger.seal(active.generation, coordinator).unwrap();
        let digest = ledger
            .write_sealed_ledger(active.generation, &Vec::<String>::new())
            .unwrap();
        ledger
            .write_terminal_with_digest(
                active.generation,
                coordinator,
                ParentLossTerminalDisposition::CancelledBeforeAdmission,
                Some(digest),
            )
            .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert!(
            ledger
                .record(active.generation)
                .unwrap()
                .unwrap()
                .closure
                .is_none()
        );
    }

    #[test]
    fn the_coordinator_lease_is_exclusive_while_held() {
        let directory = TempDir::new().unwrap();
        let ledger = ParentLossLedger::open(directory.path()).unwrap();
        let lease = ledger.acquire_coordinator_lease().unwrap().unwrap();
        assert!(ledger.acquire_coordinator_lease().unwrap().is_none());
        drop(lease);
        assert!(ledger.acquire_coordinator_lease().unwrap().is_some());
    }

    #[test]
    fn a_live_descendant_owned_by_another_user_refuses_without_signalling() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9901, 100);
        let alien = instance(9902, 200);
        admit(
            directory.path(),
            active.generation,
            "alien-descendant",
            None,
            Some(root),
        );
        write_census(
            &ledger,
            active.generation,
            &[obligation(
                active.generation,
                "alien-descendant",
                root,
                vec![descendant(alien, 9, false)],
                true,
            )],
        );
        let (source, retirer) = fake(&[(alien, 9)], &[], &[alien.pid]);
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionLive
        ));
        assert!(retirer.signals().is_empty());
        assert!(
            ledger
                .record(active.generation)
                .unwrap()
                .unwrap()
                .closure
                .is_none()
        );
    }

    #[test]
    fn a_recovery_cache_the_census_lacks_is_observed_before_it_is_dropped() {
        for live_stranger in [true, false] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let real = instance(5001, 100);
            let stranger = instance(5002, 200);
            admit(
                directory.path(),
                active.generation,
                "admitted-target",
                None,
                Some(real),
            );
            let live = obligation(
                active.generation,
                "admitted-target",
                stranger,
                Vec::new(),
                false,
            );
            let path = ledger
                .closure_recovery_dir(active.generation)
                .join("live/admitted-target.json");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            write_json(&path, &live, json_options()).unwrap();
            let alive: Vec<_> = if live_stranger {
                vec![(stranger, 501)]
            } else {
                Vec::new()
            };
            let (source, retirer) = fake(&alive, &[stranger.pid], &[]);
            let lease = acquire_lease(&ledger);
            start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");
            let notes = closure_of(&ledger, active.generation).notes;
            if live_stranger {
                assert_eq!(
                    retirer.signals(),
                    vec![(stranger.pid, "term")],
                    "kept and retired"
                );
            } else {
                assert!(retirer.signals().is_empty());
                assert!(notes.iter().any(|note| note.contains("observed gone")));
            }
        }
    }

    #[test]
    fn a_recovery_cache_rooted_in_this_journals_environment_is_kept() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(5011, 100);
        admit(directory.path(), active.generation, "env-root", None, None);
        let mut live = obligation(active.generation, "env-root", root, Vec::new(), false);
        live.sources = Some(AdmissionSourceSnapshot {
            environment_identity: Some(identity(active.generation, "env-root", root)),
            environment_journal: ledger.canonical_root().to_str().map(str::to_owned),
            ..AdmissionSourceSnapshot::default()
        });
        let path = ledger
            .closure_recovery_dir(active.generation)
            .join("live/env-root.json");
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        write_json(&path, &live, json_options()).unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");
        let closure = closure_of(&ledger, active.generation);
        assert!(
            !closure
                .notes
                .iter()
                .any(|note| note.contains("observed gone"))
        );
        assert_eq!(
            closure.summary.unwrap().exited,
            1,
            "the env-rooted obligation settled"
        );
    }

    #[test]
    fn a_damaged_recovery_cache_is_lost_identities_only_without_a_census() {
        for with_census in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let dir = ledger.generation_path(active.generation);
            fs::write(dir.join("census-refusal.json"), b"{\"schema\":1}").unwrap();
            if with_census {
                write_census(&ledger, active.generation, &[]);
            }
            let path = ledger
                .closure_recovery_dir(active.generation)
                .join("live/gone.json");
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(&path, b"{ torn").unwrap();
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
            if with_census {
                result.expect("an authoritative census stands");
                assert!(
                    closure_of(&ledger, active.generation)
                        .notes
                        .iter()
                        .any(|note| note.contains("gone") && note.contains("damaged"))
                );
            } else {
                assert!(refused(
                    &result,
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
                ));
            }
        }
    }

    #[test]
    fn a_retained_census_retires_descendants_after_root_exit_and_survives_a_lost_cache() {
        for corrupt_cache in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let root = instance(8801, 100);
            let child = instance(8802, 200);
            admit(
                directory.path(),
                active.generation,
                "lost-live-cache",
                None,
                Some(root),
            );
            write_census(
                &ledger,
                active.generation,
                &[obligation(
                    active.generation,
                    "lost-live-cache",
                    root,
                    vec![descendant(child, 501, false)],
                    true,
                )],
            );
            if corrupt_cache {
                let live = ledger
                    .closure_recovery_dir(active.generation)
                    .join("live/lost-live-cache.json");
                fs::create_dir_all(live.parent().unwrap()).unwrap();
                fs::write(&live, b"broken cache").unwrap();
            }
            let (source, retirer) = fake(&[(child, 501)], &[child.pid], &[]);
            let lease = acquire_lease(&ledger);
            start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
            assert_eq!(retirer.signals(), vec![(child.pid, "term")]);
            assert!(census(&ledger, active.generation).is_empty());
            let closure = closure_of(&ledger, active.generation);
            assert_eq!(closure.admissions[0].instance, Some(root));
            assert!(!ledger.closure_recovery_dir(active.generation).exists());
        }
    }

    #[test]
    fn the_first_reap_grace_starts_after_a_slow_kill_is_sent() {
        struct SlowKill {
            inner: FakeRetirer,
            killed_at_ms: Mutex<Option<u64>>,
        }
        impl AdmissionRetirer for SlowKill {
            fn owner(&self, pid: u32) -> ProcessOwner {
                self.inner.owner(pid)
            }
            fn signal(
                &self,
                target: ProcessInstance,
                signal: SignalKind,
            ) -> Result<(), TerminationError> {
                if matches!(signal, SignalKind::Kill) {
                    thread::sleep(Duration::from_millis(100));
                    *self.killed_at_ms.lock().unwrap() = Some(unix_millis());
                }
                self.inner.signal(target, signal)
            }
        }
        for on_descendant in [false, true] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let root = instance(8830, 100);
            let child = instance(8831, 200);
            admit(
                directory.path(),
                active.generation,
                "slow-kill",
                None,
                Some(root),
            );
            let mut live = obligation(
                active.generation,
                "slow-kill",
                root,
                if on_descendant {
                    vec![descendant(child, 501, true)]
                } else {
                    Vec::new()
                },
                true,
            );
            live.escalate_at_unix_ms = unix_millis() - 1;
            if let Some(d) = live.descendants.first_mut() {
                d.escalate_at_unix_ms = unix_millis() - 1;
            }
            write_census(&ledger, active.generation, &[live]);
            let stubborn = if on_descendant { child } else { root };
            let (source, inner) = fake(&[(stubborn, 501)], &[], &[stubborn.pid]);
            let retirer = SlowKill {
                inner,
                killed_at_ms: Mutex::new(None),
            };
            let lease = acquire_lease(&ledger);
            let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
            assert!(refused(
                &result,
                BootstrapRecoveryReason::AbandonedAdmissionLive
            ));
            let killed_at = retirer.killed_at_ms.lock().unwrap().expect("killed");
            let saved = &census(&ledger, active.generation)["slow-kill"];
            let reap = if on_descendant {
                saved.descendants[0].reap_deadline_unix_ms
            } else {
                saved.reap_deadline_unix_ms
            }
            .expect("the reap deadline is stored");
            assert!(
                reap >= killed_at + REAP_GRACE_MS,
                "on_descendant={on_descendant}"
            );
        }
    }

    #[test]
    fn a_descendant_that_loses_observability_or_uid_after_term_cannot_be_discharged_or_killed() {
        struct Changing {
            inner: FakeRetirer,
            lose_observability: bool,
        }
        impl AdmissionRetirer for Changing {
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
            admit(
                directory.path(),
                active.generation,
                "changing",
                None,
                Some(root),
            );
            let mut d = descendant(child, 501, true);
            d.escalate_at_unix_ms = unix_millis() - 1;
            write_census(
                &ledger,
                active.generation,
                &[obligation(
                    active.generation,
                    "changing",
                    root,
                    vec![d],
                    true,
                )],
            );
            let (source, inner) = fake(&[(child, 501)], &[], &[child.pid]);
            let retirer = Changing {
                inner,
                lose_observability,
            };
            let lease = acquire_lease(&ledger);
            assert!(
                start(
                    &ledger,
                    &lease,
                    &source,
                    &retirer,
                    Duration::from_millis(100)
                )
                .is_err()
            );
            assert_eq!(
                ledger.active_generation().unwrap().unwrap().generation,
                active.generation
            );
            assert!(!census(&ledger, active.generation).is_empty());
            assert_eq!(retirer.inner.signals(), vec![(child.pid, "term")]);
        }
    }

    #[test]
    fn the_first_term_gets_its_full_grace_even_if_the_census_was_captured_earlier() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(8813, 400);
        admit(
            directory.path(),
            active.generation,
            "unstarted",
            None,
            Some(root),
        );
        let mut live = obligation(active.generation, "unstarted", root, Vec::new(), false);
        live.escalate_at_unix_ms = unix_millis() - 1;
        write_census(&ledger, active.generation, &[live]);
        let (source, retirer) = fake(&[(root, 501)], &[], &[root.pid]);
        let lease = acquire_lease(&ledger);
        let before = unix_millis();
        let result = start(
            &ledger,
            &lease,
            &source,
            &retirer,
            Duration::from_millis(50),
        );
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionLive
        ));
        assert_eq!(retirer.signals(), vec![(root.pid, "term")]);
        let saved = &census(&ledger, active.generation)["unstarted"];
        assert!(saved.escalate_at_unix_ms >= before + TERM_GRACE_MS);
        assert!(!saved.escalated);
    }

    #[test]
    fn stored_timing_clamps_downward_and_a_past_reap_refuses_live() {
        assert_eq!((TERM_GRACE_MS, REAP_GRACE_MS), (12_500, 500));
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9001, 100);
        admit(
            directory.path(),
            active.generation,
            "timing",
            None,
            Some(root),
        );
        let now_ms = unix_millis();
        let mut live = obligation(active.generation, "timing", root, Vec::new(), true);
        live.escalate_at_unix_ms = now_ms + 999_999;
        write_census(&ledger, active.generation, &[live]);
        let (source, retirer) = fake(&[(root, 501)], &[], &[root.pid]);
        let lease = acquire_lease(&ledger);
        let result = start(
            &ledger,
            &lease,
            &source,
            &retirer,
            Duration::from_millis(50),
        );
        let ended = unix_millis();
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionLive
        ));
        assert!(
            census(&ledger, active.generation)["timing"].escalate_at_unix_ms
                <= ended + TERM_GRACE_MS
        );

        let mut past = obligation(active.generation, "timing", root, Vec::new(), true);
        past.escalate_at_unix_ms = now_ms - 1000;
        past.reap_deadline_unix_ms = Some(now_ms - 100);
        write_census(&ledger, active.generation, &[past]);
        let began = Instant::now();
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionLive
        ));
        assert!(
            began.elapsed() < Duration::from_secs(5),
            "a past reap is not waited out again"
        );
        assert_eq!(
            census(&ledger, active.generation)["timing"].escalate_at_unix_ms,
            now_ms - 1000,
            "a retry keeps the stored escalation instead of granting a fresh grace"
        );
    }

    #[cfg(all(test, feature = "full-tests"))]
    fn spawn_test_child(ignore_sigterm: bool) -> (std::process::Child, ProcessInstance) {
        let fixture = option_env!("CARGO_BIN_EXE_solstone-system-test-child")
            .map(PathBuf::from)
            .filter(|path| path.is_file())
            .or_else(|| {
                let current = std::env::current_exe().ok()?;
                let dir = current.parent()?.to_path_buf();
                [
                    dir.join("solstone-system-test-child"),
                    dir.parent()?.join("solstone-system-test-child"),
                ]
                .into_iter()
                .find(|candidate| candidate.is_file())
            })
            .unwrap_or_else(|| PathBuf::from("solstone-system-test-child"));
        let ready = std::env::temp_dir().join(format!(
            "solstone-outstanding-stubborn-{}-{}",
            std::process::id(),
            unique_suffix()
        ));
        let mut command = std::process::Command::new(fixture);
        if ignore_sigterm {
            command.args(["block-term-sleep", ready.to_str().unwrap()]);
        } else {
            command.arg("sleep");
        }
        let child = command
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .expect("spawn child");
        if ignore_sigterm {
            for _ in 0..500 {
                if ready.exists() {
                    break;
                }
                thread::sleep(Duration::from_millis(10));
            }
            assert!(ready.exists(), "stubborn child installed its SIGTERM mask");
            let _ = fs::remove_file(&ready);
        }
        let instance = match SystemProcessInstanceSource.inspect(child.id()) {
            InspectResult::Present { instance, .. } => instance,
            other => panic!("own child must be inspectable: {other:?}"),
        };
        (child, instance)
    }

    #[cfg(all(test, feature = "full-tests"))]
    #[test]
    fn a_real_term_blocking_child_gets_its_grace_then_is_killed() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let (mut child, inst) = spawn_test_child(true);
        let mut id = identity(active.generation, "stubborn-child", inst);
        id.uid = nix::unistd::getuid().as_raw();
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(active.generation, "stubborn-child", None, None),
        )
        .unwrap();
        acknowledge_parent_loss_admission(directory.path(), id).unwrap();
        // Reap the child as soon as it dies, so the exact identity goes away.
        let reaper = thread::spawn(move || child.wait());
        let lease = acquire_lease(&ledger);
        let began = Instant::now();
        let outcome = start(
            &ledger,
            &lease,
            &SystemProcessInstanceSource,
            &SystemAdmissionRetirer,
            AMPLE_BUDGET,
        );
        let elapsed = began.elapsed();
        let _ = reaper.join();
        assert!(outcome.is_ok(), "the eventual KILL retires it: {outcome:?}");
        assert!(
            elapsed >= Duration::from_millis(12_000),
            "waited out the grace: {elapsed:?}"
        );
        assert_eq!(
            finding(&closure_of(&ledger, active.generation), "stubborn-child"),
            Some(AdmissionFinding::Retired { escalated: true })
        );
    }

    #[test]
    fn a_retained_descendant_stays_an_obligation_while_it_lives() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9911, 100);
        let child = instance(9912, 200);
        admit(
            directory.path(),
            active.generation,
            "census-obligation",
            None,
            Some(root),
        );
        let mut d = descendant(child, 501, true);
        d.escalate_at_unix_ms = 1;
        d.reap_deadline_unix_ms = Some(1);
        d.escalated = true;
        write_census(
            &ledger,
            active.generation,
            &[obligation(
                active.generation,
                "census-obligation",
                root,
                vec![d],
                true,
            )],
        );
        let (source, retirer) = fake(&[(child, 501)], &[], &[child.pid]);
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionLive
        ));
        assert!(census(&ledger, active.generation).contains_key("census-obligation"));
        assert_eq!(
            entries(&ledger, active.generation),
            vec!["census-obligation"]
        );
    }

    // ---- descendants before signal ----

    /// A live root with a live descendant, the closer killed right after its
    /// TERM wave; then the root exits and the descendant ignores TERM. The
    /// next start must still retire the descendant.
    fn descendants_persisted_before_signal_survive_a_closer_crash() -> Result<(), String> {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9301, 100);
        let child = instance(9302, 200);
        admit(
            directory.path(),
            active.generation,
            "parent",
            None,
            Some(root),
        );
        let (source, retirer) = fake(&[(root, 501), (child, 501)], &[root.pid], &[]);
        source
            .0
            .lock()
            .unwrap()
            .children
            .insert(root.pid, vec![child.pid]);
        let lease = acquire_lease(&ledger);
        let crashed = with_stop(Stop::TermSent, || {
            start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET)
        });
        if crashed.is_ok() {
            return Err("the stop point did not stop the closer".to_owned());
        }
        retirer.signals.lock().unwrap().clear();
        // The root obeyed TERM; the descendant, reparented, still lives.
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        let child_signalled = retirer.signals().iter().any(|(pid, _)| *pid == child.pid);
        match result {
            Ok(_) if child_signalled => Ok(()),
            Err(_) => Ok(()),
            Ok(_) => Err("the next start closed while a descendant lived unsignalled".to_owned()),
        }
    }

    #[test]
    fn the_descendant_census_is_durable_before_the_first_signal() {
        descendants_persisted_before_signal_survive_a_closer_crash().unwrap();
    }

    #[test]
    fn skipping_the_descendant_persist_turns_the_crash_test_red() {
        with_fault(Fault::SkipDescendantPersist, || {
            assert!(descendants_persisted_before_signal_survive_a_closer_crash().is_err());
        });
    }

    // ---- write orderings, at each crash point ----

    #[test]
    fn a_crash_after_an_obligation_leaves_the_census_keeps_its_entry_outstanding() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9401, 100);
        admit(
            directory.path(),
            active.generation,
            "retired",
            None,
            Some(root),
        );
        let (source, retirer) = fake(&[(root, 501)], &[root.pid], &[]);
        let lease = acquire_lease(&ledger);
        assert!(
            with_stop(Stop::ObligationOut, || start(
                &ledger,
                &lease,
                &source,
                &retirer,
                AMPLE_BUDGET
            ))
            .is_err()
        );
        // ⛔ Never an entry discharged while its obligation is still held.
        assert!(census(&ledger, active.generation).is_empty());
        assert_eq!(entries(&ledger, active.generation), vec!["retired"]);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("converges");
        assert!(entries(&ledger, active.generation).is_empty());
    }

    #[test]
    fn a_crash_between_the_terminal_and_the_census_deletes_never_strands_a_started_census() {
        for point in [Stop::TerminalWritten, Stop::StartedDeleted] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let root = instance(9501, 100);
            admit(
                directory.path(),
                active.generation,
                "retired",
                None,
                Some(root),
            );
            let (source, retirer) = fake(&[(root, 501)], &[root.pid], &[]);
            let lease = acquire_lease(&ledger);
            assert!(
                with_stop(point, || start(
                    &ledger,
                    &lease,
                    &source,
                    &retirer,
                    AMPLE_BUDGET
                ))
                .is_err()
            );
            let dir = ledger.generation_path(active.generation);
            let started = dir.join("census-started.json").exists();
            let obligation_file = ledger.census_obligation_path(active.generation).exists();
            assert!(
                !started || obligation_file,
                "{point:?}: a started census without its file"
            );
            assert!(
                ledger
                    .record(active.generation)
                    .unwrap()
                    .unwrap()
                    .closure
                    .is_some()
            );
            let successor =
                start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("converges");
            assert_eq!(successor.generation, active.generation + 1);
        }
    }

    #[test]
    fn the_fold_is_persisted_before_any_legacy_cache_moves() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9601, 100);
        let child = instance(9602, 200);
        admit(
            directory.path(),
            active.generation,
            "legacy",
            None,
            Some(root),
        );
        // The released closer's shape: the obligation in its census and the
        // descendant only in the live cache (one write ahead of the census).
        write_census(
            &ledger,
            active.generation,
            &[obligation(
                active.generation,
                "legacy",
                root,
                Vec::new(),
                true,
            )],
        );
        let cache = obligation(
            active.generation,
            "legacy",
            root,
            vec![descendant(child, 501, true)],
            true,
        );
        let live = ledger
            .closure_recovery_dir(active.generation)
            .join("live/legacy.json");
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        write_json(&live, &cache, json_options()).unwrap();
        let (source, retirer) = fake(&[(child, 501)], &[child.pid], &[]);
        let lease = acquire_lease(&ledger);
        assert!(
            with_stop(Stop::FoldPersisted, || start(
                &ledger,
                &lease,
                &source,
                &retirer,
                AMPLE_BUDGET
            ))
            .is_err()
        );
        assert_eq!(
            census(&ledger, active.generation)["legacy"]
                .descendants
                .len(),
            1
        );
        assert!(
            live.exists(),
            "the cache moves only after the fold is durable"
        );
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("converges");
        assert_eq!(retirer.signals(), vec![(child.pid, "term")]);
        assert!(!ledger.closure_recovery_dir(active.generation).exists());
    }

    /// An entry that appears after the first listing, still live. Closing on
    /// the end of that listing would abandon it.
    fn a_fresh_listing_decides_the_close() -> Result<(), String> {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "early",
            None,
            Some(instance(4242, 42)),
        );
        let late = instance(9701, 97);
        let journal = directory.path().to_path_buf();
        let generation = active.generation;
        AFTER_FIRST_LISTING.with(|hook| {
            *hook.borrow_mut() = Some(Box::new(move || {
                admit(&journal, generation, "late", None, Some(late));
            }));
        });
        let (source, retirer) = fake(&[(late, 501)], &[late.pid], &[]);
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        AFTER_FIRST_LISTING.with(|hook| hook.borrow_mut().take());
        let signalled = retirer.signals().iter().any(|(pid, _)| *pid == late.pid);
        match result {
            Ok(_) if signalled => Ok(()),
            Err(_) => Ok(()),
            Ok(_) => Err("closed with a live entry the first listing missed".to_owned()),
        }
    }

    #[test]
    fn the_close_waits_for_a_fresh_empty_listing() {
        a_fresh_listing_decides_the_close().unwrap();
    }

    #[test]
    fn inferring_empty_from_the_end_of_iteration_turns_the_listing_test_red() {
        with_fault(Fault::InferEmpty, || {
            assert!(a_fresh_listing_decides_the_close().is_err());
        });
    }

    // ---- the census and its legacy markers ----

    #[test]
    fn a_lost_census_refuses_with_a_named_reason() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "a",
            None,
            Some(instance(4242, 42)),
        );
        let dir = ledger.generation_path(active.generation);
        write_json(
            dir.join("census-started.json"),
            &serde_json::json!({"schema": 1}),
            json_options(),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        for _ in 0..2 {
            let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
            assert!(refused(
                &result,
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
            ));
        }
        assert!(parent_loss_census_is_lost(&dir));
    }

    #[test]
    fn a_live_cache_one_write_behind_never_stands_in_for_a_lost_census() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9801, 100);
        admit(
            directory.path(),
            active.generation,
            "legacy",
            None,
            Some(root),
        );
        let dir = ledger.generation_path(active.generation);
        write_json(
            dir.join("census-started.json"),
            &serde_json::json!({"schema": 1}),
            json_options(),
        )
        .unwrap();
        // The census held a descendant the cache never got; then it was damaged.
        fs::write(
            ledger.census_obligation_path(active.generation),
            b"{\"schema\":1,",
        )
        .unwrap();
        let cache = obligation(active.generation, "legacy", root, Vec::new(), true);
        let live = ledger
            .closure_recovery_dir(active.generation)
            .join("live/legacy.json");
        fs::create_dir_all(live.parent().unwrap()).unwrap();
        write_json(&live, &cache, json_options()).unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
        ));
        assert!(live.exists(), "nothing is discarded from a lost census");
    }

    #[test]
    fn a_recoverable_legacy_refusal_marker_clears_and_an_unrecoverable_one_refuses() {
        for recoverable in [true, false] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            let root = instance(9851, 100);
            let child = instance(9852, 200);
            admit(
                directory.path(),
                active.generation,
                "marked",
                None,
                Some(root),
            );
            let dir = ledger.generation_path(active.generation);
            write_census(
                &ledger,
                active.generation,
                &[obligation(
                    active.generation,
                    "marked",
                    root,
                    vec![descendant(child, 501, true)],
                    true,
                )],
            );
            fs::write(
                dir.join("census-refusal.json"),
                serde_json::to_vec(&serde_json::json!({
                    "schema": 1, "generation": active.generation, "sealed_phase": "sealed",
                    "reason": "retirement root is not authorized by its source evidence",
                }))
                .unwrap(),
            )
            .unwrap();
            if !recoverable {
                fs::write(ledger.census_obligation_path(active.generation), b"{ torn").unwrap();
            }
            let (source, retirer) = fake(&[(child, 501)], &[child.pid], &[]);
            let lease = acquire_lease(&ledger);
            let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
            if recoverable {
                result.expect("a recoverable marker clears");
                assert_eq!(retirer.signals(), vec![(child.pid, "term")]);
                assert!(!dir.join("census-refusal.json").exists());
            } else {
                assert!(refused(
                    &result,
                    BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
                ));
                assert!(retirer.signals().is_empty());
                assert!(dir.join("census-refusal.json").exists());
            }
        }
    }

    #[test]
    fn a_set_aside_census_copy_is_authoritative_and_folded() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9861, 100);
        let child = instance(9862, 200);
        admit(
            directory.path(),
            active.generation,
            "copied",
            None,
            Some(root),
        );
        write_census(
            &ledger,
            active.generation,
            &[obligation(
                active.generation,
                "copied",
                root,
                vec![descendant(child, 501, true)],
                true,
            )],
        );
        let dir = ledger.generation_path(active.generation);
        fs::rename(
            ledger.census_obligation_path(active.generation),
            dir.join("census-obligation.wedged-1700000000.json"),
        )
        .unwrap();
        let (source, retirer) = fake(&[(child, 501)], &[child.pid], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("folded and closed");
        assert_eq!(retirer.signals(), vec![(child.pid, "term")]);
        assert!(
            !dir.join("census-obligation.wedged-1700000000.json")
                .exists()
        );
    }

    // ---- every unsettled generation, and the boot ----

    #[test]
    fn a_spurious_higher_generation_and_an_older_open_one_are_both_closed() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let live = instance(5555, 55);
        admit(
            directory.path(),
            active.generation,
            "live-worker",
            None,
            Some(live),
        );
        fs::create_dir_all(ledger.generation_path(active.generation + 3)).unwrap();
        fs::write(ledger.root_path().join("generations/not-a-number"), b"x").unwrap();
        let (source, retirer) = fake(&[(live, 501)], &[live.pid], &[]);
        let lease = acquire_lease(&ledger);
        let successor = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        assert_eq!(successor.generation, active.generation + 4);
        assert_eq!(retirer.signals(), vec![(live.pid, "term")]);
        assert!(
            closure_of(&ledger, active.generation)
                .summary
                .unwrap()
                .retired
                == 1
        );
        assert!(
            ledger
                .record(active.generation + 3)
                .unwrap()
                .unwrap()
                .closure
                .is_some()
        );
    }

    type BootOutcome = (
        Vec<(u32, &'static str)>,
        Result<ActiveGeneration, ParentLossLedgerError>,
        Option<ClosureSummary>,
    );

    fn boot_case(boot: Option<Option<String>>, supervisor_boot: Option<u128>) -> BootOutcome {
        let directory = TempDir::new().unwrap();
        let ledger = ParentLossLedger::open(directory.path()).unwrap();
        let mut supervisor = instance(10, 1);
        supervisor.birth = supervisor.birth.on_linux_boot(supervisor_boot);
        let active = ledger.reserve_generation(supervisor, []).unwrap();
        ledger.initialize_record(&active).unwrap();
        let coordinator = instance(20, 2);
        let active = ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .unwrap();
        let active = ledger
            .mark_admitting(active.generation, coordinator)
            .unwrap();
        if supervisor_boot.is_some() {
            let _ = fs::remove_file(boot_path(&ledger, active.generation));
        }
        let live = instance(5656, 56);
        admit(
            directory.path(),
            active.generation,
            "live",
            None,
            Some(live),
        );
        let (source, mut retirer) = fake(&[(live, 501)], &[live.pid], &[]);
        retirer.boot = boot;
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        let summary = ledger
            .record(active.generation)
            .unwrap()
            .and_then(|record| record.closure)
            .and_then(|closure| closure.summary);
        (retirer.signals(), result, summary)
    }

    #[test]
    fn a_changed_boot_settles_the_whole_generation_at_once() {
        if current_boot_id().is_none() {
            return;
        }
        let (signals, result, summary) = boot_case(
            Some(Some("0123456789abcdef0123456789abcdef".to_owned())),
            None,
        );
        result.expect("closed");
        assert!(signals.is_empty(), "nothing from another boot is signalled");
        let summary = summary.unwrap();
        assert!(summary.boot_changed);
        assert_eq!(summary.total(), 0, "entries were never read");
    }

    #[test]
    fn an_unreadable_boot_id_settles_nothing() {
        let (signals, result, summary) = boot_case(Some(None), None);
        result.expect("closed entry by entry");
        assert_eq!(signals, vec![(5656, "term")]);
        assert!(!summary.unwrap().boot_changed);
    }

    #[test]
    fn a_linux_supervisor_birth_from_another_boot_settles_the_generation() {
        let current = "8b9f3c1e77a24d0b9e612f4c05aad317";
        let other = parse_linux_boot_id("0123456789abcdef0123456789abcdef");
        let (signals, result, summary) = boot_case(Some(Some(current.to_owned())), other);
        result.expect("closed");
        assert!(signals.is_empty());
        assert!(summary.unwrap().boot_changed);
        let (signals, _, _) =
            boot_case(Some(Some(current.to_owned())), parse_linux_boot_id(current));
        assert_eq!(
            signals,
            vec![(5656, "term")],
            "the same boot is observed entry by entry"
        );
    }

    #[test]
    fn reservation_records_its_boot_only_from_a_successful_read() {
        let directory = TempDir::new().unwrap();
        let ledger = ParentLossLedger::open(directory.path()).unwrap();
        let active = ledger.reserve_generation(instance(10, 1), []).unwrap();
        let path = boot_path(&ledger, active.generation);
        match current_boot_id() {
            Some(boot) => {
                let record: BootRecord = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
                assert_eq!((record.schema, record.boot), (1, boot));
            }
            None => assert!(!path.exists()),
        }
    }

    // ---- what stays flat ----

    fn write_exited_history(
        ledger: &ParentLossLedger,
        generation: ParentLossGeneration,
        count: usize,
    ) {
        let root = admission_directory(ledger, generation);
        for index in 0..count {
            let launch_id = format!("worker-{index:08x}");
            let dir = root.join(&launch_id);
            fs::create_dir_all(&dir).unwrap();
            let id = identity(
                generation,
                &launch_id,
                instance(100_000 + index as u32, 1_000 + index as u64),
            );
            let intent = AdmissionIntent::new(generation, launch_id.clone(), None, None);
            fs::write(
                dir.join("intent.json"),
                serde_json::to_vec(&intent).unwrap(),
            )
            .unwrap();
            fs::write(
                dir.join("spawn-identity.json"),
                serde_json::to_vec(&AdmissionSpawnIdentity {
                    schema: 1,
                    identity: id.clone(),
                })
                .unwrap(),
            )
            .unwrap();
            fs::write(
                dir.join("result.json"),
                serde_json::to_vec(&AdmissionResult {
                    schema: 1,
                    identity: Some(id),
                    state: AdmissionResultState::Admitted,
                })
                .unwrap(),
            )
            .unwrap();
        }
    }

    /// Close a synthetic legacy-shaped generation of `completed` exited
    /// entries and 3 live ones, then time the start after it.
    fn close_then_restart(completed: usize) -> (Duration, Duration) {
        let directory = tempfile::Builder::new()
            .prefix("pl-outstanding-")
            .tempdir_in("/var/tmp")
            .unwrap();
        let (ledger, active) = open_generation_at(directory.path());
        write_exited_history(&ledger, active.generation, completed);
        let live: Vec<_> = (0..3)
            .map(|i| (instance(900_001 + i, 9_000 + u64::from(i)), 501))
            .collect();
        for (i, (child, _)) in live.iter().enumerate() {
            admit(
                directory.path(),
                active.generation,
                &format!("live-{i}"),
                None,
                Some(*child),
            );
        }
        let pids: Vec<u32> = live.iter().map(|(i, _)| i.pid).collect();
        let (source, retirer) = fake(&live, &pids, &[]);
        let lease = acquire_lease(&ledger);
        let began = Instant::now();
        start(&ledger, &lease, &source, &retirer, Duration::from_secs(600))
            .expect("one start closes it");
        let closing = began.elapsed();
        assert_eq!(
            summary_of(&ledger, active.generation).exited,
            completed as u64
        );
        // The next generation dies too, empty; its start must not re-pay the first.
        let began = Instant::now();
        start(&ledger, &lease, &source, &retirer, Duration::from_secs(600)).expect("restart");
        (closing, began.elapsed())
    }

    fn open_generation_at(path: &Path) -> (ParentLossLedger, ActiveGeneration) {
        let ledger = ParentLossLedger::open(path).unwrap();
        let active = ledger.reserve_generation(instance(10, 1), []).unwrap();
        ledger.initialize_record(&active).unwrap();
        let coordinator = instance(20, 2);
        let active = ledger
            .persist_coordinator_identity(active.generation, coordinator)
            .unwrap();
        let active = ledger
            .mark_admitting(active.generation, coordinator)
            .unwrap();
        (ledger, active)
    }

    #[test]
    fn a_closed_history_costs_the_next_start_nothing() {
        let (small_close, small_next) = close_then_restart(1_000);
        let (large_close, large_next) = close_then_restart(100_000);
        eprintln!(
            "outstanding: close K=1k {small_close:?}, K=100k {large_close:?}; \
             next start K=1k {small_next:?}, K=100k {large_next:?}"
        );
        assert!(
            large_next <= small_next * 3 + Duration::from_millis(50),
            "the start after a closed 100k history grew: {small_next:?} vs {large_next:?}"
        );
        // A legacy-shaped set is still outstanding work, read once: linear,
        // and closed by one start (`close_then_restart` asserts the one).
    }

    // ---- the prune ----

    #[test]
    fn the_prune_deletes_only_what_settled_generations_below_the_highest_left() {
        let directory = TempDir::new().unwrap();
        let (ledger, first) = open_generation(&directory);
        admit(
            directory.path(),
            first.generation,
            "a",
            None,
            Some(instance(4242, 42)),
        );
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        let second = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).unwrap();
        ledger.initialize_record(&second).unwrap();
        admit(
            directory.path(),
            second.generation,
            "b",
            None,
            Some(instance(4343, 43)),
        );
        let old = ledger.generation_path(first.generation);
        fs::create_dir_all(old.join("admissions/leftover")).unwrap();
        assert!(old.join("discharged/a").is_dir());
        prune_settled_generations(&ledger);
        assert!(!old.join("discharged").exists());
        assert!(!old.join("admissions").exists());
        assert!(
            ledger.record_path(first.generation).is_file(),
            "the record stays"
        );
        assert_eq!(
            entries(&ledger, second.generation),
            vec!["b"],
            "the highest generation is never pruned"
        );
    }

    // ---- what the independent review found ----

    #[test]
    fn a_root_that_turns_live_after_a_blind_read_is_never_signalled_unread() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9871, 100);
        write_census(
            &ledger,
            active.generation,
            &[obligation(
                active.generation,
                "flicker",
                root,
                Vec::new(),
                false,
            )],
        );
        let (source, retirer) = fake(&[(root, 501)], &[root.pid], &[]);
        source.0.lock().unwrap().blind_reads.insert(root.pid, 1);
        let lease = acquire_lease(&ledger);
        let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
        assert!(refused(
            &result,
            BootstrapRecoveryReason::AbandonedAdmissionUnverifiable
        ));
        assert!(
            retirer.signals().is_empty(),
            "signalled before its subtree was read"
        );
        assert!(census(&ledger, active.generation).contains_key("flicker"));
        // Once it reads cleanly, the next start retires it.
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("converges");
        assert_eq!(retirer.signals(), vec![(root.pid, "term")]);
    }

    #[test]
    fn every_live_identity_of_an_entry_has_its_subtree_retired() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let g = active.generation;
        let (acked, recorded, child) = (instance(9881, 1), instance(9882, 2), instance(9883, 3));
        write_parent_loss_admission_intent(
            directory.path(),
            &AdmissionIntent::new(g, "split", None, None),
        )
        .unwrap();
        acknowledge_parent_loss_admission(directory.path(), identity(g, "split", acked)).unwrap();
        write_parent_loss_admission_result(
            directory.path(),
            g,
            "split",
            &AdmissionResult {
                schema: 1,
                identity: Some(identity(g, "split", recorded)),
                state: AdmissionResultState::Admitted,
            },
        )
        .unwrap();
        let live = [(acked, 501), (recorded, 501), (child, 501)];
        let (source, retirer) = fake(&live, &[acked.pid, recorded.pid, child.pid], &[]);
        // The child hangs off the identity that is not the obligation's root.
        source
            .0
            .lock()
            .unwrap()
            .children
            .insert(acked.pid, vec![child.pid]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");
        assert_eq!(
            retirer.signals(),
            vec![
                (acked.pid, "term"),
                (recorded.pid, "term"),
                (child.pid, "term")
            ]
        );
    }

    #[test]
    fn a_kill_that_never_landed_is_sent_again_by_the_next_start() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let root = instance(9891, 100);
        admit(
            directory.path(),
            active.generation,
            "unkilled",
            None,
            Some(root),
        );
        let mut live = obligation(active.generation, "unkilled", root, Vec::new(), true);
        live.escalate_at_unix_ms = unix_millis() - 2_000;
        live.escalated = true;
        live.reap_deadline_unix_ms = Some(unix_millis() - 1_000);
        write_census(&ledger, active.generation, &[live]);
        let (source, retirer) = fake(&[(root, 501)], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("the KILL lands this time");
        assert_eq!(
            retirer.signals(),
            vec![(root.pid, "kill"), (root.pid, "term")]
        );
        assert_eq!(
            finding(&closure_of(&ledger, active.generation), "unkilled"),
            Some(AdmissionFinding::Retired { escalated: true })
        );
    }

    #[test]
    fn a_directory_where_a_drop_belongs_is_damage_not_doubt() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let g = active.generation;
        // The entry directory exists before any drop but the intent, as a
        // launch makes it; a directory then squats where the intent belongs.
        fs::create_dir_all(admission_directory(&ledger, g).join("odd/intent.json")).unwrap();
        write_parent_loss_admission_spawn_identity(
            directory.path(),
            &identity(g, "odd", instance(4242, 42)),
        )
        .unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("closed");
        let closure = closure_of(&ledger, g);
        assert_eq!(closure.summary.unwrap().exited, 1);
        assert!(
            closure
                .notes
                .iter()
                .any(|note| note.contains("odd") && note.contains("intent.json"))
        );
    }

    // ---- the second review ----

    fn a_destroyed_sole_identity_never_heals() -> Result<(), String> {
        for (drop, bytes) in [
            (
                "spawn-identity.json",
                b"{\"schema\":1,\"identity\":{".to_vec(),
            ),
            (
                "acknowledgement.json",
                br#"{"schema":2,"identity":{}}"#.to_vec(),
            ),
            ("result.json", Vec::new()),
        ] {
            let directory = TempDir::new().unwrap();
            let (ledger, active) = open_generation(&directory);
            write_parent_loss_admission_intent(
                directory.path(),
                &AdmissionIntent::new(active.generation, "destroyed", None, None),
            )
            .unwrap();
            let path = admission_directory(&ledger, active.generation)
                .join("destroyed")
                .join(drop);
            if bytes.is_empty() {
                fs::create_dir_all(&path).unwrap();
            } else {
                fs::write(&path, &bytes).unwrap();
            }
            let (source, retirer) = fake(&[], &[], &[]);
            let lease = acquire_lease(&ledger);
            let result = start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET);
            if !refused(
                &result,
                BootstrapRecoveryReason::AbandonedAdmissionUnverifiable,
            ) {
                return Err(format!(
                    "{drop}: a destroyed sole identity healed: {result:?}"
                ));
            }
            if entries(&ledger, active.generation) != vec!["destroyed"] {
                return Err(format!("{drop}: the entry was discharged"));
            }
        }
        Ok(())
    }

    #[test]
    fn a_destroyed_sole_identity_is_kept_and_refuses() {
        a_destroyed_sole_identity_never_heals().unwrap();
    }

    #[test]
    fn healing_a_destroyed_identity_turns_the_test_red() {
        with_fault(Fault::HealDamagedIdentity, || {
            assert!(a_destroyed_sole_identity_never_heals().is_err());
        });
    }

    #[test]
    fn a_torn_intent_alone_still_heals_and_a_boot_change_settles_a_destroyed_identity() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let dir = admission_directory(&ledger, active.generation);
        fs::create_dir_all(dir.join("torn-intent")).unwrap();
        fs::write(dir.join("torn-intent/intent.json"), b"{\"schema\":").unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("an intent alone heals");
        assert_eq!(summary_of(&ledger, active.generation).never_acknowledged, 1);

        if current_boot_id().is_none() {
            return;
        }
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let dir = admission_directory(&ledger, active.generation).join("destroyed");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("spawn-identity.json"), b"{ torn").unwrap();
        let (source, mut retirer) = fake(&[], &[], &[]);
        retirer.boot = Some(Some("0123456789abcdef0123456789abcdef".to_owned()));
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("boot-settled");
        assert!(summary_of(&ledger, active.generation).boot_changed);
    }

    #[test]
    fn boot_settlement_needs_a_valid_id_and_on_macos_a_changed_boot_time() {
        let other = "8b9f3c1e-77a2-4d0b-9e61-2f4c05aad317";
        let current = parse_boot_uuid("0123456789abcdef0123456789abcdef").unwrap();
        let record = |boot: &str, boot_time| BootRecord {
            schema: 1,
            boot: boot.to_owned(),
            boot_time,
        };
        assert_eq!(
            parse_boot_uuid("00000000-0000-0000-0000-000000000000"),
            None
        );
        assert_eq!(parse_boot_uuid("not-a-boot"), None);
        assert_eq!(parse_boot_uuid("8b9f3c1e77a2-4d0b-9e61-2f4c05aad317"), None);
        assert!(
            boot_settles(&record(other, None), &current, None, false),
            "linux: id alone"
        );
        assert!(!boot_settles(
            &record(&current, None),
            &current,
            None,
            false
        ));
        assert!(!boot_settles(
            &record("garbage", None),
            &current,
            None,
            false
        ));
        assert!(
            !boot_settles(&record(other, None), &current, Some(9), true),
            "mac: no recorded time"
        );
        assert!(
            !boot_settles(&record(other, Some(9)), &current, Some(9), true),
            "mac: same time"
        );
        assert!(
            !boot_settles(&record(other, Some(8)), &current, None, true),
            "mac: unreadable time"
        );
        assert!(boot_settles(
            &record(other, Some(8)),
            &current,
            Some(9),
            true
        ));
    }

    #[test]
    fn file_shaped_state_directories_are_set_aside_not_refused() {
        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        admit(
            directory.path(),
            active.generation,
            "gone",
            None,
            Some(instance(4242, 42)),
        );
        let generation_dir = ledger.generation_path(active.generation);
        fs::write(generation_dir.join("discharged"), b"x").unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET).expect("a file-shaped trash heals");
        assert!(generation_dir.join("discharged/gone").is_dir());

        let directory = TempDir::new().unwrap();
        let (ledger, active) = open_generation(&directory);
        let generation_dir = ledger.generation_path(active.generation);
        fs::write(generation_dir.join("admissions"), b"x").unwrap();
        let (source, retirer) = fake(&[], &[], &[]);
        let lease = acquire_lease(&ledger);
        start(&ledger, &lease, &source, &retirer, AMPLE_BUDGET)
            .expect("a file-shaped admissions heals");
        assert!(
            closure_of(&ledger, active.generation)
                .notes
                .iter()
                .any(|note| note.contains("admissions was a file"))
        );
    }
}
