// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

#[cfg(not(windows))]
use std::panic::{self, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, LockResult, Mutex, MutexGuard, TryLockResult};
use std::thread;
use std::time::{Duration, Instant};

#[cfg(test)]
use std::{cell::Cell, thread_local};

use axum::http::StatusCode;
use axum::response::Response;
use serde::Serialize;
use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation, Clock};

use crate::{
    archive_operation::{ArchiveBegin, ArchiveBeginRefusal, ArchiveCancellationRequest},
    response,
    validation::RefusedReason,
};

pub const HANDOFF_TTL: Duration = Duration::from_secs(30 * 60);

const TERMINAL: &[&str] = &["done", "error", "degraded", "needs_subscription", "refused"];

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct Operation {
    pub kind: String,
    pub phase: String,
    pub reason_code: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    pub recording_failure: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report: Option<serde_json::Value>,
    pub portal_url: Option<String>,
}

pub struct Slot {
    pub view: Operation,
    // Hosted wait capability; never serialized (the nonce only appears inside portal_url).
    pub nonce: Option<String>,
    // restore_hosted wait only; never an Operation field so it cannot leak into JSON.
    pub restore_key: Option<String>,
    pub started: Instant,
    pub generation: u64,
    pub(crate) worker_active: bool,
    pub(crate) archive_cancellation: Option<ArchiveCancellation>,
    pub(crate) handoff_poll: Option<ArchiveCancellation>,
    pub(crate) pending_handoff_terminal: Option<Terminal>,
    #[cfg(windows)]
    pub(crate) cleanup: Option<PendingCleanup>,
}

#[cfg(windows)]
pub(crate) struct PendingCleanup {
    pub(crate) owners: Vec<solstone_core_system::process::BoundedHelperCleanup>,
    terminal: Terminal,
}

#[cfg(windows)]
pub(crate) fn observe_cleanup(current: &mut Slot) {
    use solstone_core_system::process::HelperCleanupStatus;
    let settled = current.cleanup.as_ref().is_some_and(|pending| {
        pending
            .owners
            .iter()
            .all(|owner| matches!(owner.observe(), HelperCleanupStatus::Quiescent))
    });
    if settled && let Some(pending) = current.cleanup.take() {
        finish_or_defer(current, pending.terminal);
    }
}

pub(crate) fn apply_terminal(current: &mut Slot, terminal: Terminal) {
    current.view.phase = terminal.phase;
    current.view.reason_code = terminal.reason_code;
    current.view.detail = terminal.detail;
    current.view.guidance = terminal.guidance;
    current.view.recording_failure = terminal.recording_failure;
    current.view.report = terminal.report;
    current.view.portal_url = None;
    current.nonce = None;
    current.restore_key = None;
    current.pending_handoff_terminal = None;
}

pub(crate) fn finish_or_defer(current: &mut Slot, terminal: Terminal) {
    if current.pending_handoff_terminal.is_some() {
        return;
    }
    if let Some(cancellation) = &current.handoff_poll {
        cancellation.cancel();
        current.view.phase = "cancelling".into();
        current.view.reason_code = terminal.reason_code.clone();
        current.view.portal_url = None;
        current.nonce = None;
        current.restore_key = None;
        current.pending_handoff_terminal = Some(terminal);
    } else {
        apply_terminal(current, terminal);
    }
}

pub struct OperationState {
    pub(crate) current: Option<Slot>,
    pub(crate) archive: crate::archive_worker::ArchiveCustody,
    pub(crate) closed: bool,
    pub writes_in_flight: usize,
    maintenance_in_flight: usize,
    pub(crate) recovery_controls_in_flight: usize,
    pub(crate) threads: Vec<thread::JoinHandle<()>>,
    pub(crate) settlement_worker: Option<thread::JoinHandle<()>>,
    pub(crate) join_batches: Vec<JoinBatch>,
    pub(crate) failed_joins: Vec<JoinBatch>,
    pub(crate) cancellation: ArchiveCancellation,
}

impl OperationState {
    pub fn archive_returns(
        &self,
    ) -> impl Iterator<Item = &crate::archive_worker::RejectedArchiveReturn> {
        self.archive.retained()
    }

    fn new(current: Option<Slot>) -> Self {
        Self {
            current,
            archive: Default::default(),
            closed: false,
            writes_in_flight: 0,
            maintenance_in_flight: 0,
            recovery_controls_in_flight: 0,
            threads: Vec::new(),
            settlement_worker: None,
            join_batches: Vec::new(),
            failed_joins: Vec::new(),
            cancellation: ArchiveCancellation::new(),
        }
    }
}

/// Queued tasks capture only shared custody. The real native handles remain
/// here if the embedding app's executor cannot start its join task.
pub(crate) struct JoinBatch {
    pub(crate) task: Option<tokio::task::JoinHandle<()>>,
    pub(crate) owners: Arc<Mutex<Vec<thread::JoinHandle<()>>>>,
    pub(crate) panics: Arc<Mutex<Vec<Box<dyn std::any::Any + Send>>>>,
    pub(crate) failure: Option<tokio::task::JoinError>,
}

pub struct OperationShared {
    mutex: Mutex<OperationState>,
    cvar: Condvar,
    wait_mutex: Mutex<()>,
    stopping: AtomicBool,
    cancellation: ArchiveCancellation,
    joining: std::sync::atomic::AtomicUsize,
    join_failure: AtomicBool,
}

impl OperationShared {
    fn from_state(state: OperationState) -> Self {
        let cancellation = state.cancellation.clone();
        Self {
            mutex: Mutex::new(state),
            cvar: Condvar::new(),
            wait_mutex: Mutex::new(()),
            stopping: AtomicBool::new(false),
            cancellation,
            joining: std::sync::atomic::AtomicUsize::new(0),
            join_failure: AtomicBool::new(false),
        }
    }
    pub fn lock(&self) -> LockResult<MutexGuard<'_, OperationState>> {
        match self.mutex.lock() {
            Ok(mut guard) => {
                self.apply_stop(&mut guard);
                Ok(guard)
            }
            Err(poisoned) => {
                let mut guard = poisoned.into_inner();
                self.apply_stop(&mut guard);
                Err(std::sync::PoisonError::new(guard))
            }
        }
    }

    pub fn try_lock(&self) -> TryLockResult<MutexGuard<'_, OperationState>> {
        match self.mutex.try_lock() {
            Ok(mut guard) => {
                self.apply_stop(&mut guard);
                Ok(guard)
            }
            Err(std::sync::TryLockError::Poisoned(poisoned)) => {
                let mut guard = poisoned.into_inner();
                self.apply_stop(&mut guard);
                Err(std::sync::TryLockError::Poisoned(
                    std::sync::PoisonError::new(guard),
                ))
            }
            Err(error) => Err(error),
        }
    }

    pub(crate) fn request_stop(&self) {
        self.stopping.store(true, Ordering::Release);
        self.cancellation.cancel();
        // Capability erasure is opportunistic; every later lock synchronizes
        // the closed state before an admission can inspect it.
        drop(self.try_lock());
        self.notify_waiters();
    }

    pub(crate) fn is_stopping(&self) -> bool {
        self.stopping.load(Ordering::Acquire)
    }

    pub(crate) fn begin_join(&self, count: usize) {
        self.joining.fetch_add(count, Ordering::AcqRel);
    }

    pub(crate) fn finish_join(&self, count: usize, failed: bool) {
        if failed {
            self.join_failure.store(true, Ordering::Release);
        }
        self.joining.fetch_sub(count, Ordering::AcqRel);
        self.notify_waiters();
    }

    pub(crate) fn joining(&self) -> bool {
        self.joining.load(Ordering::Acquire) != 0
    }

    pub(crate) fn join_failed(&self) -> bool {
        self.join_failure.load(Ordering::Acquire)
    }

    fn apply_stop(&self, state: &mut OperationState) {
        if !self.stopping.load(Ordering::Acquire) {
            return;
        }
        state.closed = true;
        if let Some(current) = state.current.as_mut() {
            if let Some(cancellation) = &current.archive_cancellation {
                cancellation.cancel();
            }
            if let Some(cancellation) = &current.handoff_poll {
                cancellation.cancel();
            }
            current.nonce = None;
            current.restore_key = None;
        }
    }

    pub fn is_poisoned(&self) -> bool {
        self.mutex.is_poisoned()
    }

    pub(crate) fn notify_waiters(&self) {
        self.cvar.notify_all();
    }

    pub(crate) fn wait_timeout(&self, timeout: Duration) {
        // The wake mutex is used only for waiting, never for domain IO. A
        // timeout must not reacquire the operation mutex held by a real worker.
        if let Ok(guard) = self.wait_mutex.try_lock() {
            drop(self.cvar.wait_timeout(guard, timeout));
        }
    }
}

pub type SharedOperationSlot = Arc<OperationShared>;

#[cfg(test)]
thread_local! {
    static INSTANCE_ALLOCATIONS: Cell<usize> = const { Cell::new(0) };
}

pub fn new_slot() -> SharedOperationSlot {
    Arc::new(OperationShared::from_state(OperationState::new(None)))
}

pub fn is_terminal(phase: &str) -> bool {
    TERMINAL.contains(&phase)
}

pub fn running_phase(kind: &str) -> &'static str {
    match kind {
        "enable" | "enable_hosted" => "setting_up",
        "restore" | "restore_hosted" | "offload_restore" => "restoring",
        "rotate" => "rotating",
        "teardown" => "tearing_down",
        _ => "setting_up",
    }
}

fn hosted_wait_expired(slot: &Slot) -> bool {
    if slot.worker_active {
        return false;
    }
    #[cfg(windows)]
    if slot.cleanup.is_some() {
        return false;
    }
    !is_terminal(&slot.view.phase)
        && matches!(slot.view.kind.as_str(), "enable_hosted" | "restore_hosted")
        && slot.nonce.is_some()
        && slot.started.elapsed() > HANDOFF_TTL
}

fn expire_hosted_wait_in_place(slot: &mut Slot) {
    if !hosted_wait_expired(slot) {
        return;
    }
    finish_or_defer(slot, Terminal::error("expired"));
}

fn observe(slot: &mut Option<Slot>) -> Option<&Slot> {
    if let Some(current) = slot.as_mut() {
        #[cfg(windows)]
        observe_cleanup(current);
        expire_hosted_wait_in_place(current);
    }
    slot.as_ref()
}

pub fn is_busy(slot: &SharedOperationSlot) -> bool {
    #[cfg(windows)]
    {
        use solstone_core_system::process::{
            HelperAdmissionStatus, observe_bounded_helper_admission,
        };
        if !matches!(
            observe_bounded_helper_admission(),
            HelperAdmissionStatus::Ready
        ) {
            return true;
        }
        let Ok(mut guard) = slot.try_lock() else {
            return true;
        };
        crate::archive_worker::observe_retained(&mut guard);
        return guard.archive.is_busy()
            || observe(&mut guard.current)
                .is_some_and(|current| !is_terminal(&current.view.phase));
    }
    #[cfg(not(windows))]
    {
        let Ok(mut guard) = slot.lock() else {
            return true;
        };
        guard.archive.is_busy()
            || observe(&mut guard.current).is_some_and(|slot| !is_terminal(&slot.view.phase))
    }
}

#[cfg(windows)]
pub(crate) fn observed_current(slot: &SharedOperationSlot) -> Result<Option<Operation>, ()> {
    let mut guard = slot.try_lock().map_err(|_| ())?;
    crate::archive_worker::observe_retained(&mut guard);
    Ok(observe(&mut guard.current).map(|slot| slot.view.clone()))
}

#[cfg(windows)]
pub(crate) fn observed_generation(
    slot: &SharedOperationSlot,
    generation: Option<u64>,
) -> Result<Option<Operation>, ()> {
    let mut guard = slot.try_lock().map_err(|_| ())?;
    if guard.current.as_ref().map(|current| current.generation) != generation {
        return Err(());
    }
    crate::archive_worker::observe_retained(&mut guard);
    Ok(observe(&mut guard.current).map(|current| current.view.clone()))
}

#[cfg(all(test, windows))]
pub(crate) fn worker_or_cleanup_active(slot: &SharedOperationSlot, generation: u64) -> bool {
    let Ok(guard) = slot.try_lock() else {
        return true;
    };
    guard.current.as_ref().is_some_and(|current| {
        current.generation == generation && (current.worker_active || current.cleanup.is_some())
    })
}

pub(crate) fn start_worker_locked(guard: &mut OperationState, generation: u64) -> bool {
    if guard.closed || guard.archive.is_busy() {
        return false;
    }
    let Some(current) = guard.current.as_mut() else {
        return false;
    };
    if current.generation != generation
        || is_terminal(&current.view.phase)
        || current
            .archive_cancellation
            .as_ref()
            .is_some_and(|stop| stop.is_cancelled())
        || current.worker_active
        || current.pending_handoff_terminal.is_some()
    {
        return false;
    }
    #[cfg(windows)]
    if current.cleanup.is_some() {
        return false;
    }
    current.worker_active = true;
    true
}

#[allow(dead_code)]
pub(crate) fn start_worker(slot: &SharedOperationSlot, generation: u64) -> bool {
    #[cfg(windows)]
    let Ok(mut guard) = slot.try_lock() else {
        return false;
    };
    #[cfg(not(windows))]
    let mut guard = slot.lock().expect("operation slot lock");
    start_worker_locked(&mut guard, generation)
}

#[cfg(windows)]
pub(crate) fn finish_worker(
    slot: &SharedOperationSlot,
    generation: u64,
    terminal: Terminal,
    owners: Vec<solstone_core_system::process::BoundedHelperCleanup>,
) {
    let Ok(mut guard) = slot.lock() else {
        return;
    };
    if guard.archive.is_busy() {
        return;
    }
    let Some(current) = guard.current.as_mut() else {
        return;
    };
    if current.generation != generation || !current.worker_active {
        return;
    }
    current.worker_active = false;
    if owners.iter().any(|owner| {
        matches!(
            owner.observe(),
            solstone_core_system::process::HelperCleanupStatus::Pending
        )
    }) {
        current.view.phase = "cleanup_pending".into();
        current.view.reason_code = Some("cleanup_pending".into());
        current.view.portal_url = None;
        current.nonce = None;
        current.restore_key = None;
        current.cleanup = Some(PendingCleanup { owners, terminal });
    } else {
        finish_or_defer(current, terminal);
    }
    drop(guard);
    slot.notify_waiters();
}

#[cfg(not(windows))]
pub(crate) fn finish_worker(slot: &SharedOperationSlot, generation: u64, terminal: Terminal) {
    let mut guard = slot.lock().expect("operation slot lock");
    if guard.archive.is_busy() {
        return;
    }
    let Some(current) = guard.current.as_mut() else {
        return;
    };
    if current.generation != generation || !current.worker_active {
        return;
    }
    current.worker_active = false;
    finish_or_defer(current, terminal);
    drop(guard);
    slot.notify_waiters();
}

pub(crate) fn start_handoff_poll_locked(
    guard: &mut OperationState,
    generation: u64,
) -> Option<ArchiveCancellation> {
    if guard.closed || guard.archive.is_busy() {
        return None;
    }
    let current = guard.current.as_mut()?;
    if current.generation != generation {
        return None;
    }
    expire_hosted_wait_in_place(current);
    if is_terminal(&current.view.phase)
        || current.worker_active
        || current.handoff_poll.is_some()
        || current.pending_handoff_terminal.is_some()
        || current.nonce.is_none()
        || !matches!(
            current.view.kind.as_str(),
            "enable_hosted" | "restore_hosted"
        )
    {
        return None;
    }
    let cancellation = ArchiveCancellation::new_child(&guard.cancellation);
    current.handoff_poll = Some(cancellation.clone());
    Some(cancellation)
}

#[allow(dead_code)]
pub(crate) fn start_handoff_poll(
    slot: &SharedOperationSlot,
    generation: u64,
) -> Option<ArchiveCancellation> {
    let mut guard = slot.lock().expect("operation slot lock");
    start_handoff_poll_locked(&mut guard, generation)
}

pub(crate) fn finish_handoff_poll(slot: &SharedOperationSlot, generation: u64) {
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.current.as_mut() else {
        return;
    };
    if current.generation != generation || current.handoff_poll.take().is_none() {
        return;
    }
    if current.worker_active {
        drop(guard);
        slot.notify_waiters();
        return;
    }
    #[cfg(windows)]
    if current.cleanup.is_some() {
        drop(guard);
        slot.notify_waiters();
        return;
    }
    if let Some(terminal) = current.pending_handoff_terminal.take() {
        apply_terminal(current, terminal);
    }
    drop(guard);
    slot.notify_waiters();
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn handoff_poll_active(slot: &SharedOperationSlot) -> bool {
    slot.try_lock()
        .map(|guard| {
            guard
                .current
                .as_ref()
                .is_some_and(|current| current.handoff_poll.is_some())
        })
        .unwrap_or(true)
}

/// Only the poll still owning this nonce may publish a response failure.
pub(crate) fn fail_handoff_poll(
    slot: &SharedOperationSlot,
    generation: u64,
    nonce: &str,
    reason: &str,
) {
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.current.as_mut() else {
        return;
    };
    if current.generation != generation
        || current.handoff_poll.is_none()
        || current.nonce.as_deref() != Some(nonce)
        || current.worker_active
        || current.pending_handoff_terminal.is_some()
    {
        return;
    }
    finish_or_defer(current, Terminal::error(reason));
}

#[cfg(any(not(windows), test))]
pub fn current(slot: &SharedOperationSlot) -> Option<Operation> {
    let mut guard = slot.lock().expect("operation slot lock");
    #[cfg(windows)]
    crate::archive_worker::observe_retained(&mut guard);
    observe(&mut guard.current).map(|slot| slot.view.clone())
}

pub fn busy_response() -> Response {
    response::error(
        StatusCode::BAD_REQUEST,
        "that action isn't available in the current state.",
        "backup_busy",
        "",
    )
}

pub struct Begin {
    pub generation: u64,
}

pub fn begin(
    slot: &SharedOperationSlot,
    kind: &str,
    portal_url: Option<String>,
    nonce: Option<String>,
    restore_key: Option<String>,
) -> Result<Begin, Response> {
    let mut guard = lock_for_begin(slot).map_err(|()| busy_response())?;
    let cancellation = crate::recovery_owner::owns_recovery(kind)
        .then(|| ArchiveCancellation::new_child(&guard.cancellation));
    Ok(install_slot(
        &mut guard.current,
        kind,
        portal_url,
        nonce,
        restore_key,
        cancellation,
    ))
}

pub(crate) fn begin_archive(
    slot: &SharedOperationSlot,
    kind: &str,
    clock: &dyn Clock,
) -> Result<ArchiveBegin, ArchiveBeginRefusal> {
    let mut guard = lock_for_begin(slot).map_err(|()| ArchiveBeginRefusal::Busy)?;
    let cancellation = ArchiveCancellation::new_child(&guard.cancellation);
    let budget = ArchiveBudget::new_cancellable(clock, cancellation.clone())
        .map_err(ArchiveBeginRefusal::Admission)?;
    let begun = install_slot(
        &mut guard.current,
        kind,
        None,
        None,
        None,
        Some(cancellation),
    );
    Ok(ArchiveBegin::new(begun.generation, budget))
}

pub(crate) fn lock_for_begin(
    slot: &SharedOperationSlot,
) -> Result<MutexGuard<'_, OperationState>, ()> {
    #[cfg(windows)]
    {
        use solstone_core_system::process::{
            HelperAdmissionStatus, observe_bounded_helper_admission,
        };
        if !matches!(
            observe_bounded_helper_admission(),
            HelperAdmissionStatus::Ready
        ) {
            return Err(());
        }
    }
    #[cfg(windows)]
    let mut guard = slot.try_lock().map_err(|_| ())?;
    #[cfg(not(windows))]
    let mut guard = slot.lock().map_err(|_| ())?;
    #[cfg(windows)]
    crate::archive_worker::observe_retained(&mut guard);
    if guard.closed
        || slot.joining()
        || slot.join_failed()
        || !guard.join_batches.is_empty()
        || guard.writes_in_flight != 0
        || guard.archive.is_busy()
        || observe(&mut guard.current).is_some_and(|slot| !is_terminal(&slot.view.phase))
    {
        return Err(());
    }
    Ok(guard)
}

pub(crate) fn install_slot(
    current: &mut Option<Slot>,
    kind: &str,
    portal_url: Option<String>,
    nonce: Option<String>,
    restore_key: Option<String>,
    archive_cancellation: Option<ArchiveCancellation>,
) -> Begin {
    let generation = current
        .as_ref()
        .map(|slot| slot.generation.wrapping_add(1))
        .unwrap_or(1);
    let view = Operation {
        kind: kind.to_owned(),
        phase: running_phase(kind).to_owned(),
        reason_code: None,
        detail: None,
        guidance: None,
        recording_failure: None,
        report: None,
        portal_url,
    };
    *current = Some(Slot {
        view,
        nonce,
        restore_key,
        started: Instant::now(),
        generation,
        worker_active: false,
        archive_cancellation,
        handoff_poll: None,
        pending_handoff_terminal: None,
        #[cfg(windows)]
        cleanup: None,
    });
    Begin { generation }
}

pub(crate) fn request_archive_cancellation(
    slot: &SharedOperationSlot,
    generation: u64,
) -> ArchiveCancellationRequest {
    #[cfg(windows)]
    let Ok(mut guard) = slot.try_lock() else {
        return ArchiveCancellationRequest::Refused;
    };
    #[cfg(not(windows))]
    let mut guard = slot.lock().expect("operation slot lock");
    let owning_archive = guard.archive.is_busy();
    let Some(current) = guard.current.as_mut() else {
        return ArchiveCancellationRequest::Refused;
    };
    if current.generation != generation || is_terminal(&current.view.phase) {
        return ArchiveCancellationRequest::Refused;
    }
    #[cfg(windows)]
    if current.cleanup.is_some() {
        return ArchiveCancellationRequest::Refused;
    }
    let Some(cancellation) = current.archive_cancellation.as_ref() else {
        return ArchiveCancellationRequest::Refused;
    };
    cancellation.cancel();
    if current.worker_active || owning_archive {
        current.view.phase = "cancelling".into();
        ArchiveCancellationRequest::Requested
    } else {
        apply_terminal(current, Terminal::error("cancelled"));
        ArchiveCancellationRequest::BeforeStart
    }
}

pub fn finish(
    slot: &SharedOperationSlot,
    generation: u64,
    phase: impl Into<String>,
    reason_code: Option<String>,
    recording_failure: Option<String>,
) {
    finish_terminal(
        slot,
        generation,
        Terminal {
            phase: phase.into(),
            reason_code,
            detail: None,
            guidance: None,
            recording_failure,
            report: None,
        },
    );
}

fn finish_terminal(slot: &SharedOperationSlot, generation: u64, terminal: Terminal) -> bool {
    let mut guard = slot.lock().expect("operation slot lock");
    if guard.archive.is_busy() {
        return false;
    }
    let Some(current) = guard.current.as_mut() else {
        return false;
    };
    if current.generation != generation || is_terminal(&current.view.phase) {
        return false;
    }
    if current.worker_active {
        return false;
    }
    if current.pending_handoff_terminal.is_some() {
        return false;
    }
    #[cfg(windows)]
    if current.cleanup.is_some() {
        return false;
    }
    finish_or_defer(current, terminal);
    true
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Terminal {
    pub phase: String,
    pub reason_code: Option<String>,
    pub detail: Option<String>,
    pub guidance: Option<String>,
    pub recording_failure: Option<String>,
    pub report: Option<serde_json::Value>,
}

impl Terminal {
    pub fn done() -> Self {
        Self::phase("done", None)
    }

    pub fn error(reason_code: impl Into<String>) -> Self {
        Self {
            phase: "error".into(),
            reason_code: Some(reason_code.into()),
            detail: None,
            guidance: None,
            recording_failure: None,
            report: None,
        }
    }

    pub fn tool_error(error: solstone_core_backup_runtime::ClosedToolError) -> Self {
        Self {
            phase: "error".into(),
            reason_code: Some(error.to_string()),
            detail: Some(error.detail().to_owned()),
            guidance: Some(error.guidance().to_owned()),
            recording_failure: None,
            report: None,
        }
    }

    pub fn phase(phase: impl Into<String>, reason_code: Option<String>) -> Self {
        Self {
            phase: phase.into(),
            reason_code,
            detail: None,
            guidance: None,
            recording_failure: None,
            report: None,
        }
    }

    pub fn restore(
        phase: impl Into<String>,
        reason_code: Option<String>,
        recording_failure: Option<String>,
    ) -> Self {
        Self {
            phase: phase.into(),
            reason_code,
            detail: None,
            guidance: None,
            recording_failure,
            report: None,
        }
    }
}

#[cfg(not(windows))]
pub fn spawn_worker<F>(slot: SharedOperationSlot, generation: u64, work: F) -> bool
where
    F: FnOnce() -> Terminal + Send + 'static,
{
    let mut guard = match slot.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.closed || !start_worker_locked(&mut guard, generation) {
        if let Some(current) = guard.current.as_mut()
            && current.generation == generation
            && !is_terminal(&current.view.phase)
            && !current.worker_active
        {
            finish_or_defer(current, Terminal::error("failed"));
        }
        drop(guard);
        slot.notify_waiters();
        return false;
    }
    let worker_slot = slot.clone();
    match spawn_thread(move || {
        let terminal = panic::catch_unwind(AssertUnwindSafe(work))
            .unwrap_or_else(|_| Terminal::error("failed"));
        finish_worker(&worker_slot, generation, terminal);
    }) {
        Ok(handle) => {
            guard.threads.push(handle);
            drop(guard);
            slot.notify_waiters();
            true
        }
        Err(_) => {
            if let Some(current) = guard.current.as_mut()
                && current.generation == generation
            {
                current.worker_active = false;
                finish_or_defer(current, Terminal::error("failed"));
            }
            drop(guard);
            slot.notify_waiters();
            false
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmitWrite {
    DuringOperation,
    IdleOnly,
    Maintenance,
    /// Logical recovery control; blocks new operation admission while joining.
    RecoveryControl,
}

pub struct WriteGuard {
    slot: SharedOperationSlot,
    maintenance: bool,
    recovery_control: bool,
}

impl WriteGuard {
    pub(crate) fn controls_recovery_of(&self, slot: &SharedOperationSlot) -> bool {
        self.recovery_control && Arc::ptr_eq(&self.slot, slot)
    }
}

impl Drop for WriteGuard {
    fn drop(&mut self) {
        let mut guard = match self.slot.lock() {
            Ok(g) => g,
            Err(e) => e.into_inner(),
        };
        guard.writes_in_flight = guard.writes_in_flight.saturating_sub(1);
        if self.maintenance {
            guard.maintenance_in_flight = guard.maintenance_in_flight.saturating_sub(1);
        }
        if self.recovery_control {
            guard.recovery_controls_in_flight = guard.recovery_controls_in_flight.saturating_sub(1);
        }
        drop(guard);
        self.slot.notify_waiters();
    }
}

pub fn admit_write(slot: &SharedOperationSlot, mode: AdmitWrite) -> Result<WriteGuard, Response> {
    #[cfg(windows)]
    if !matches!(
        mode,
        AdmitWrite::DuringOperation | AdmitWrite::RecoveryControl
    ) {
        use solstone_core_system::process::{
            HelperAdmissionStatus, observe_bounded_helper_admission,
        };
        if !matches!(
            observe_bounded_helper_admission(),
            HelperAdmissionStatus::Ready
        ) {
            return Err(busy_response());
        }
    }
    #[cfg(windows)]
    let mut guard = slot.try_lock().map_err(|_| busy_response())?;
    #[cfg(not(windows))]
    let mut guard = slot.lock().map_err(|_| busy_response())?;
    if guard.closed {
        return Err(busy_response());
    }
    if !matches!(
        mode,
        AdmitWrite::DuringOperation | AdmitWrite::RecoveryControl
    ) {
        #[cfg(windows)]
        crate::archive_worker::observe_retained(&mut guard);
        let conflicting_write = match mode {
            AdmitWrite::IdleOnly => guard.writes_in_flight != 0,
            AdmitWrite::Maintenance => guard.writes_in_flight != guard.maintenance_in_flight,
            AdmitWrite::DuringOperation | AdmitWrite::RecoveryControl => false,
        };
        if conflicting_write
            || guard.archive.is_busy()
            || observe(&mut guard.current).is_some_and(|slot| !is_terminal(&slot.view.phase))
        {
            return Err(busy_response());
        }
    }
    guard.writes_in_flight = guard.writes_in_flight.saturating_add(1);
    if mode == AdmitWrite::Maintenance {
        guard.maintenance_in_flight = guard.maintenance_in_flight.saturating_add(1);
    }
    if mode == AdmitWrite::RecoveryControl {
        guard.recovery_controls_in_flight = guard.recovery_controls_in_flight.saturating_add(1);
    }
    Ok(WriteGuard {
        slot: Arc::clone(slot),
        maintenance: mode == AdmitWrite::Maintenance,
        recovery_control: mode == AdmitWrite::RecoveryControl,
    })
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    static FAIL_WORKER_START: Cell<bool> = const { Cell::new(false) };
}

pub(crate) fn spawn_thread(
    work: impl FnOnce() + Send + 'static,
) -> std::io::Result<thread::JoinHandle<()>> {
    #[cfg(all(test, feature = "full-tests"))]
    if FAIL_WORKER_START.with(|fail| fail.replace(false)) {
        return Err(std::io::Error::other(
            "injected backup worker creation failure",
        ));
    }
    thread::Builder::new().spawn(work)
}

#[cfg(all(test, feature = "full-tests"))]
pub(crate) fn fail_next_worker_start() {
    FAIL_WORKER_START.with(|fail| fail.set(true));
}

#[allow(dead_code)]
pub fn mint_hex() -> Result<String, getrandom::Error> {
    #[cfg(test)]
    INSTANCE_ALLOCATIONS.with(|count| count.set(count.get().saturating_add(1)));
    mint_hex_from_csprng()
}

pub fn mint_capability() -> Result<String, getrandom::Error> {
    mint_hex_from_csprng()
}

fn mint_hex_from_csprng() -> Result<String, getrandom::Error> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
}

#[cfg(test)]
pub fn reset_instance_allocations() {
    INSTANCE_ALLOCATIONS.with(|count| count.set(0));
}

#[cfg(test)]
pub fn instance_allocations() -> usize {
    INSTANCE_ALLOCATIONS.with(Cell::get)
}

pub fn portal_url(
    base: &str,
    nonce: &str,
    instance: &str,
    assertion: &str,
    ca_pubkey: &str,
) -> String {
    solstone_core_sol_link::home_reach::service_enable_portal_url(
        base, "backup", nonce, instance, assertion, ca_pubkey,
    )
}

pub fn restore_portal_url(base: &str, nonce: &str) -> String {
    format!(
        "{}/enable/backup?nonce={nonce}&intent=restore",
        base.trim_end_matches('/')
    )
}

pub struct HandoffMatch {
    pub generation: u64,
    pub kind: String,
    pub restore_key: Option<String>,
}

pub enum HandoffError {
    Invalid,
    Expired,
}

pub fn match_handoff(
    slot: &SharedOperationSlot,
    nonce: &str,
    expected_generation: Option<u64>,
) -> Result<HandoffMatch, HandoffError> {
    let mut guard = slot.lock().expect("operation slot lock");
    let Some(current) = guard.current.as_mut() else {
        return Err(HandoffError::Invalid);
    };
    if expected_generation.is_some_and(|generation| generation != current.generation) {
        return Err(HandoffError::Invalid);
    }
    expire_hosted_wait_in_place(current);
    if is_terminal(&current.view.phase) || current.pending_handoff_terminal.is_some() {
        return if matches!(
            current.view.reason_code.as_deref(),
            Some("expired") | Some("restore_prepare_expired")
        ) {
            Err(HandoffError::Expired)
        } else {
            Err(HandoffError::Invalid)
        };
    }
    if !matches!(
        current.view.kind.as_str(),
        "enable_hosted" | "restore_hosted"
    ) || current.nonce.as_deref() != Some(nonce)
    {
        return Err(HandoffError::Invalid);
    }
    let _ = current.nonce.take();
    if let Some(cancellation) = &current.handoff_poll {
        cancellation.cancel();
    }
    Ok(HandoffMatch {
        generation: current.generation,
        kind: current.view.kind.clone(),
        restore_key: current.restore_key.clone(),
    })
}

pub fn mark_needs_subscription(slot: &SharedOperationSlot, generation: u64) {
    finish(slot, generation, "needs_subscription", None, None);
}

pub fn mark_refused(slot: &SharedOperationSlot, generation: u64, reason: RefusedReason) {
    finish(
        slot,
        generation,
        "refused",
        Some(reason.code().into()),
        None,
    );
}

pub fn mark_expired(slot: &SharedOperationSlot, generation: u64) {
    finish(slot, generation, "error", Some("expired".into()), None);
}

pub fn mark_prepare_lease_expired(slot: &SharedOperationSlot, generation: u64) {
    finish(
        slot,
        generation,
        "error",
        Some("restore_prepare_expired".into()),
        None,
    );
}

pub fn mark_cancelled(slot: &SharedOperationSlot, generation: u64) -> bool {
    finish_terminal(slot, generation, Terminal::error("cancelled"))
}

pub fn nonce_for_generation(slot: &SharedOperationSlot, generation: u64) -> Option<String> {
    let guard = slot.lock().expect("operation slot lock");
    let current = guard.current.as_ref()?;
    (current.generation == generation
        && !is_terminal(&current.view.phase)
        && current.view.kind == "restore_hosted")
        .then(|| current.nonce.clone())
        .flatten()
}

pub fn generation_of(slot: &SharedOperationSlot) -> Option<u64> {
    slot.lock()
        .expect("operation slot lock")
        .current
        .as_ref()
        .map(|slot| slot.generation)
}

#[cfg(all(test, feature = "full-tests"))]
pub fn backdate_started(slot: &SharedOperationSlot, age: Duration) {
    let mut guard = slot.lock().expect("operation slot lock");
    if let Some(current) = guard.current.as_mut() {
        current.started = Instant::now().checked_sub(age).unwrap_or_else(Instant::now);
    }
}

#[cfg(test)]
mod tests {
    use super::terminal_tests::admitted_slot;
    use super::{
        HandoffError, mark_prepare_lease_expired, match_handoff, new_slot, portal_url,
        restore_portal_url,
    };

    fn parse_portal_url(url: &str) -> (&str, &str, &str, Vec<(&str, &str)>) {
        let (scheme, remainder) = url.split_once("://").expect("scheme separator");
        let authority_end = remainder.find('/').expect("authority/path separator");
        let (authority, path_and_query) = remainder.split_at(authority_end);
        let (path, query) = path_and_query
            .split_once('?')
            .expect("path/query separator");

        let mut query_pairs = query
            .split('&')
            .map(|pair| pair.split_once('=').expect("query key/value separator"))
            .collect::<Vec<_>>();
        query_pairs.sort_unstable();

        assert!(
            query_pairs.windows(2).all(|pair| pair[0].0 != pair[1].0),
            "duplicate query key"
        );

        (scheme, authority, path, query_pairs)
    }

    #[test]
    fn portal_url_uses_exact_backup_handoff_route_and_query() {
        let nonce = "alpha-nonce-1";
        let instance = "beta-instance-2";
        let assertion = "test-assertion";
        let ca_pubkey = "test-ca-pubkey";
        let url = portal_url(
            "http://portal.example.test:8123/",
            nonce,
            instance,
            assertion,
            ca_pubkey,
        );

        let (scheme, authority, path, query_pairs) = parse_portal_url(&url);

        assert_eq!(scheme, "http");
        assert_eq!(authority, "portal.example.test:8123");
        assert_eq!(path, "/enable/backup");
        assert_eq!(
            query_pairs,
            vec![
                ("assertion", assertion),
                ("ca_pubkey", ca_pubkey),
                ("instance", instance),
                ("nonce", nonce),
            ]
        );
    }

    #[test]
    fn restore_portal_url_uses_restore_intent_without_instance() {
        let nonce = "alpha-nonce-1";
        let url = restore_portal_url("http://portal.example.test:8123/", nonce);

        let (_, _, path, query_pairs) = parse_portal_url(&url);

        assert_eq!(path, "/enable/backup");
        assert_eq!(query_pairs, vec![("intent", "restore"), ("nonce", nonce)]);
    }

    #[test]
    fn match_handoff_classifies_prepare_lease_expiry_as_expired_not_invalid() {
        let slot = new_slot();
        slot.lock().unwrap().current = Some(admitted_slot(
            "restore_hosted",
            1,
            None,
            Some("nonce-1".into()),
            None,
        ));
        mark_prepare_lease_expired(&slot, 1);

        let result = match_handoff(&slot, "nonce-1", None);

        assert!(matches!(result, Err(HandoffError::Expired)));
    }
}

#[cfg(test)]
mod terminal_tests {
    use super::*;

    pub(super) fn running_slot() -> SharedOperationSlot {
        Arc::new(OperationShared::from_state(OperationState::new(Some(
            admitted_slot(
                "restore_hosted",
                2,
                Some("https://example.test/restore".into()),
                Some("test-nonce".into()),
                Some("test-key".into()),
            ),
        ))))
    }

    pub(super) fn admitted_slot(
        kind: &str,
        generation: u64,
        portal_url: Option<String>,
        nonce: Option<String>,
        restore_key: Option<String>,
    ) -> Slot {
        Slot {
            view: Operation {
                kind: kind.into(),
                phase: running_phase(kind).into(),
                reason_code: None,
                detail: None,
                guidance: None,
                recording_failure: None,
                report: None,
                portal_url,
            },
            nonce,
            restore_key,
            started: Instant::now(),
            generation,
            worker_active: false,
            archive_cancellation: None,
            handoff_poll: None,
            pending_handoff_terminal: None,
            #[cfg(windows)]
            cleanup: None,
        }
    }

    pub(super) fn failure() -> Terminal {
        Terminal {
            phase: "degraded".into(),
            reason_code: Some("tool_failed".into()),
            detail: Some("tool detail".into()),
            guidance: Some("next step".into()),
            recording_failure: Some("recording failed".into()),
            report: Some(serde_json::json!({"files_restored":1, "bytes_restored":7})),
        }
    }

    pub(super) fn assert_failure(slot: &SharedOperationSlot) {
        let guard = slot.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        assert_eq!(current.view.phase, "degraded");
        assert_eq!(current.view.reason_code.as_deref(), Some("tool_failed"));
        assert_eq!(current.view.detail.as_deref(), Some("tool detail"));
        assert_eq!(current.view.guidance.as_deref(), Some("next step"));
        assert_eq!(
            current.view.recording_failure.as_deref(),
            Some("recording failed")
        );
        assert!(current.view.portal_url.is_none());
        assert_eq!(current.view.report, failure().report);
        assert!(current.nonce.is_none());
        assert!(current.restore_key.is_none());
    }

    #[test]
    fn complete_terminal_preserves_payload_and_clears_secrets() {
        let slot = running_slot();
        finish_terminal(&slot, 2, failure());
        assert_failure(&slot);
    }

    #[test]
    fn complete_terminal_ignores_absent_stale_and_terminal_slots() {
        let absent = new_slot();
        finish_terminal(&absent, 2, failure());
        assert!(absent.lock().unwrap().current.is_none());

        let slot = running_slot();
        let original = slot.lock().unwrap().current.as_ref().unwrap().view.clone();
        finish_terminal(&slot, 1, failure());
        assert_eq!(
            slot.lock().unwrap().current.as_ref().unwrap().view,
            original
        );
        assert!(
            slot.lock()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .restore_key
                .is_some()
        );
        finish_terminal(&slot, 2, failure());
        finish_terminal(&slot, 2, Terminal::done());
        assert_failure(&slot);
    }

    #[cfg(windows)]
    #[test]
    fn complete_terminal_respects_worker_and_cleanup_ownership() {
        let slot = running_slot();
        let original = slot.lock().unwrap().current.as_ref().unwrap().view.clone();
        slot.lock().unwrap().current.as_mut().unwrap().worker_active = true;
        finish_terminal(&slot, 2, failure());
        assert_eq!(
            slot.lock().unwrap().current.as_ref().unwrap().view,
            original
        );
        {
            let mut guard = slot.lock().unwrap();
            let current = guard.current.as_mut().unwrap();
            current.worker_active = false;
            current.cleanup = Some(PendingCleanup {
                owners: Vec::new(),
                terminal: failure(),
            });
        }
        finish_terminal(&slot, 2, Terminal::done());
        let guard = slot.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        assert_eq!(current.view, original);
        assert!(current.cleanup.is_some());
        assert!(current.nonce.is_some() && current.restore_key.is_some());
    }

    #[test]
    fn active_worker_refuses_other_terminal_transitions_until_actual_settlement() {
        let slot = running_slot();
        let original = slot.lock().unwrap().current.as_ref().unwrap().view.clone();
        assert!(start_worker(&slot, 2));
        assert!(!start_worker(&slot, 2));
        assert!(!start_worker(&slot, 1));
        slot.lock().unwrap().current.as_mut().unwrap().started =
            Instant::now() - HANDOFF_TTL - Duration::from_secs(1);
        assert!(!mark_cancelled(&slot, 2));
        mark_expired(&slot, 2);
        finish_terminal(&slot, 2, Terminal::done());
        assert_eq!(current(&slot).unwrap(), original);
        assert!(is_busy(&slot));
        assert!(begin(&slot, "rotate", None, None, None).is_err());
        settle(&slot, 1, Terminal::done());
        assert!(slot.lock().unwrap().current.as_ref().unwrap().worker_active);
        settle(&slot, 2, failure());
        assert_failure(&slot);
        #[cfg(not(windows))]
        assert!(!is_busy(&slot));

        #[cfg(windows)]
        let next = {
            let mut guard = slot.lock().unwrap();
            let settled = guard.current.as_ref().unwrap();
            assert!(!settled.worker_active);
            assert!(settled.cleanup.is_none());
            let generation = settled.generation.wrapping_add(1);
            guard.current = Some(admitted_slot("restore", generation, None, None, None));
            generation
        };
        #[cfg(not(windows))]
        let next = begin(&slot, "restore", None, None, None)
            .unwrap()
            .generation;
        assert!(start_worker(&slot, next));
        let current_view = current(&slot).unwrap();
        assert!(current_view.report.is_none());
        assert!(
            serde_json::to_value(&current_view)
                .unwrap()
                .get("report")
                .is_none()
        );
        let mut stale = Terminal::error("stale");
        stale.report = Some(serde_json::json!({"stale":true}));
        settle(&slot, 2, stale);
        assert_eq!(current(&slot).unwrap(), current_view);
        assert!(slot.lock().unwrap().current.as_ref().unwrap().worker_active);
        settle(&slot, next, Terminal::done());
    }

    fn settle(slot: &SharedOperationSlot, generation: u64, terminal: Terminal) {
        #[cfg(windows)]
        finish_worker(slot, generation, terminal, Vec::new());
        #[cfg(not(windows))]
        finish_worker(slot, generation, terminal);
    }

    #[test]
    fn cancellation_reports_only_an_applied_current_transition() {
        let absent = new_slot();
        assert!(!mark_cancelled(&absent, 2));
        assert!(!start_worker(&absent, 2));
        let slot = running_slot();
        assert!(!mark_cancelled(&slot, 1));
        assert!(mark_cancelled(&slot, 2));
        assert!(!mark_cancelled(&slot, 2));
        assert!(!start_worker(&slot, 2));
        let guard = slot.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        assert_eq!(current.view.reason_code.as_deref(), Some("cancelled"));
        assert!(current.nonce.is_none() && current.restore_key.is_none());
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(not(windows))]
mod worker_tests {
    use super::terminal_tests::{assert_failure, failure, running_slot};
    use super::*;

    fn wait_for_worker(handle: thread::JoinHandle<()>) {
        let (completed, observed) = std::sync::mpsc::channel();
        thread::spawn(move || completed.send(handle.join()).unwrap());
        observed
            .recv_timeout(Duration::from_secs(5))
            .expect("worker finished")
            .expect("worker did not panic");
    }

    fn assert_no_worker_starts(slot: SharedOperationSlot, generation: u64) {
        let (entered, observed) = std::sync::mpsc::channel();
        assert!(!spawn_worker(slot, generation, move || {
            entered.send(()).unwrap();
            Terminal::done()
        }));
        assert_eq!(
            observed.recv_timeout(Duration::from_secs(5)),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected)
        );
    }

    #[test]
    fn ordinary_worker_preserves_complete_terminal() {
        let slot = running_slot();
        assert!(spawn_worker(slot.clone(), 2, failure));
        let handle = slot.lock().unwrap().threads.pop().unwrap();
        wait_for_worker(handle);
        assert_failure(&slot);
    }

    #[test]
    fn ordinary_worker_preserves_panic_failure() {
        let slot = running_slot();
        assert!(spawn_worker(slot.clone(), 2, || panic!(
            "test worker panic"
        )));
        let handle = slot.lock().unwrap().threads.pop().unwrap();
        wait_for_worker(handle);
        let guard = slot.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        assert_eq!(current.view.phase, "error");
        assert_eq!(current.view.reason_code.as_deref(), Some("failed"));
        assert!(current.view.detail.is_none() && current.view.guidance.is_none());
        assert!(current.view.portal_url.is_none());
        assert!(current.nonce.is_none() && current.restore_key.is_none());
    }

    #[test]
    fn ordinary_worker_holds_the_slot_until_its_closure_settles() {
        let slot = running_slot();
        let (entered, observed) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        assert!(spawn_worker(slot.clone(), 2, move || {
            entered.send(()).unwrap();
            released.recv_timeout(Duration::from_secs(5)).unwrap();
            failure()
        }));
        let handle = slot.lock().unwrap().threads.pop().unwrap();
        observed.recv_timeout(Duration::from_secs(5)).unwrap();
        slot.lock().unwrap().current.as_mut().unwrap().started =
            Instant::now() - HANDOFF_TTL - Duration::from_secs(1);
        assert!(!mark_cancelled(&slot, 2));
        mark_expired(&slot, 2);
        finish_terminal(&slot, 2, Terminal::done());
        assert!(is_busy(&slot));
        assert_eq!(current(&slot).unwrap().phase, "restoring");
        assert!(begin(&slot, "teardown", None, None, None).is_err());
        assert_no_worker_starts(slot.clone(), 2);
        release.send(()).unwrap();
        wait_for_worker(handle);
        assert_failure(&slot);
        assert!(!is_busy(&slot));
    }

    #[test]
    fn refused_worker_admission_starts_no_closure() {
        let absent = new_slot();
        assert_no_worker_starts(absent, 2);
        let slot = running_slot();
        assert_no_worker_starts(slot.clone(), 1);
        assert!(mark_cancelled(&slot, 2));
        assert_no_worker_starts(slot.clone(), 2);
        assert_eq!(
            current(&slot).unwrap().reason_code.as_deref(),
            Some("cancelled")
        );
    }

    #[test]
    fn failed_thread_creation_settles_only_its_admitted_generation() {
        let slot = running_slot();
        FAIL_WORKER_START.with(|fail| fail.set(true));
        assert_no_worker_starts(slot.clone(), 2);
        let current = current(&slot).unwrap();
        assert_eq!(current.phase, "error");
        assert_eq!(current.reason_code.as_deref(), Some("failed"));
        assert!(current.portal_url.is_none());
        assert!(!slot.lock().unwrap().current.as_ref().unwrap().worker_active);
        assert!(!is_busy(&slot));
        let next = begin(&slot, "rotate", None, None, None).unwrap().generation;
        assert!(spawn_worker(slot.clone(), next, Terminal::done));
        let handle = slot.lock().unwrap().threads.pop().unwrap();
        wait_for_worker(handle);
        assert_eq!(super::current(&slot).unwrap().phase, "done");
    }

    #[test]
    fn failed_archive_worker_creation_cannot_cancel_its_successor() {
        struct ArchiveClock;
        impl Clock for ArchiveClock {
            fn now_unix(&self) -> i64 {
                100
            }
            fn iso_week(&self) -> u8 {
                1
            }
        }
        let slot = new_slot();
        let (generation, budget) = begin_archive(&slot, "offload_approve", &ArchiveClock)
            .unwrap_or_else(|error| panic!("archive admission: {error:?}"))
            .into_parts();
        let old_flag = budget.cancellation().unwrap().clone();
        FAIL_WORKER_START.with(|fail| fail.set(true));
        assert_no_worker_starts(slot.clone(), generation);
        assert_eq!(
            current(&slot).unwrap().reason_code.as_deref(),
            Some("failed")
        );
        assert!(!is_busy(&slot));
        let (next, next_budget) = begin_archive(&slot, "offload_approve", &ArchiveClock)
            .unwrap_or_else(|error| panic!("archive admission: {error:?}"))
            .into_parts();
        old_flag.cancel();
        assert!(!next_budget.cancellation().unwrap().is_cancelled());
        assert_eq!(
            request_archive_cancellation(&slot, generation),
            ArchiveCancellationRequest::Refused
        );
        assert_eq!(
            request_archive_cancellation(&slot, next),
            ArchiveCancellationRequest::BeforeStart
        );
    }
}

#[cfg(test)]
mod handoff_state_tests {
    use super::terminal_tests::{admitted_slot, assert_failure, failure};
    use super::*;

    fn admitted_poll() -> SharedOperationSlot {
        Arc::new(OperationShared::from_state(OperationState::new(Some(
            admitted_slot(
                "restore_hosted",
                2,
                Some("portal".into()),
                Some("nonce".into()),
                Some("key".into()),
            ),
        ))))
    }

    #[test]
    fn pending_handoff_blocks_admission_and_preserves_original_stop() {
        let slot = admitted_poll();
        let flag = start_handoff_poll(&slot, 2).unwrap();
        assert!(start_handoff_poll(&slot, 2).is_none());
        assert!(start_handoff_poll(&slot, 1).is_none());
        assert!(!flag.is_cancelled());
        assert!(mark_cancelled(&slot, 2));
        assert!(flag.is_cancelled());
        assert!(!mark_cancelled(&slot, 2));
        assert!(!start_worker(&slot, 2));
        assert!(!is_terminal(&current(&slot).unwrap().phase));
        assert!(
            slot.lock()
                .unwrap()
                .current
                .as_ref()
                .unwrap()
                .nonce
                .is_none()
        );
        fail_handoff_poll(&slot, 2, "nonce", "expired");
        finish_handoff_poll(&slot, 1);
        assert_eq!(current(&slot).unwrap().phase, "cancelling");
        finish_handoff_poll(&slot, 2);
        assert_eq!(
            current(&slot).unwrap().reason_code.as_deref(),
            Some("cancelled")
        );
        let next = 3;
        slot.lock().unwrap().current = Some(admitted_slot("restore", next, None, None, None));
        finish_handoff_poll(&slot, 2);
        fail_handoff_poll(&slot, 2, "nonce", "failed");
        assert_eq!(generation_of(&slot), Some(next));
        assert!(!is_terminal(&current(&slot).unwrap().phase));
    }

    #[test]
    fn poll_and_worker_settle_independently_with_complete_worker_facts() {
        for poll_first in [false, true] {
            let slot = admitted_poll();
            let flag = start_handoff_poll(&slot, 2).unwrap();
            let matched =
                match_handoff(&slot, "nonce", Some(2)).unwrap_or_else(|_| panic!("match"));
            assert_eq!(matched.generation, 2);
            assert!(flag.is_cancelled());
            assert!(start_worker(&slot, 2));
            if poll_first {
                finish_handoff_poll(&slot, 2);
                assert!(!is_terminal(&current(&slot).unwrap().phase));
            }
            #[cfg(windows)]
            finish_worker(&slot, 2, failure(), Vec::new());
            #[cfg(not(windows))]
            finish_worker(&slot, 2, failure());
            if !poll_first {
                assert!(!is_terminal(&current(&slot).unwrap().phase));
                assert!(!mark_cancelled(&slot, 2));
                fail_handoff_poll(&slot, 2, "nonce", "expired");
                finish_handoff_poll(&slot, 2);
            }
            assert_failure(&slot);
        }
    }
}

#[cfg(all(test, windows))]
mod cleanup_state_tests {
    use super::terminal_tests::admitted_slot;
    use super::*;

    #[test]
    fn poll_and_native_cleanup_each_hold_the_complete_terminal() {
        for poll_first in [false, true] {
            let slot = Arc::new(OperationShared::from_state(OperationState::new(Some(
                admitted_slot("restore_hosted", 2, None, Some("nonce".into()), None),
            ))));
            start_handoff_poll(&slot, 2).unwrap();
            match_handoff(&slot, "nonce", Some(2)).unwrap_or_else(|_| panic!("match"));
            slot.lock().unwrap().current.as_mut().unwrap().cleanup = Some(PendingCleanup {
                owners: Vec::new(),
                terminal: super::terminal_tests::failure(),
            });
            if poll_first {
                finish_handoff_poll(&slot, 2);
            } else {
                observe_cleanup(slot.lock().unwrap().current.as_mut().unwrap());
                assert!(!is_terminal(
                    &slot.lock().unwrap().current.as_ref().unwrap().view.phase
                ));
            }
            if poll_first {
                observe_cleanup(slot.lock().unwrap().current.as_mut().unwrap());
            } else {
                finish_handoff_poll(&slot, 2);
            }
            super::terminal_tests::assert_failure(&slot);
        }
    }

    #[test]
    fn archive_stop_preserves_worker_and_separate_cleanup_ownership() {
        let flag = ArchiveCancellation::new();
        let mut admitted = admitted_slot("offload_approve", 2, None, None, None);
        admitted.archive_cancellation = Some(flag.clone());
        let slot = Arc::new(OperationShared::from_state(OperationState::new(Some(
            admitted,
        ))));
        assert!(start_worker(&slot, 2));
        assert_eq!(
            request_archive_cancellation(&slot, 2),
            ArchiveCancellationRequest::Requested
        );
        assert!(flag.is_cancelled());
        assert!(worker_or_cleanup_active(&slot, 2));
        assert!(!mark_cancelled(&slot, 2));
        {
            let mut guard = slot.lock().unwrap();
            let current = guard.current.as_mut().unwrap();
            current.worker_active = false;
            current.view.phase = "cleanup_pending".into();
            current.cleanup = Some(PendingCleanup {
                owners: Vec::new(),
                terminal: Terminal::done(),
            });
        }
        assert_eq!(
            request_archive_cancellation(&slot, 2),
            ArchiveCancellationRequest::Refused
        );
        let guard = slot.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        assert_eq!(current.view.phase, "cleanup_pending");
        assert!(current.cleanup.is_some());
    }

    #[test]
    fn active_worker_keeps_ownership_across_cancel_and_hosted_expiry() {
        let slot = new_slot();
        let generation = 1;
        slot.lock().unwrap().current = Some(admitted_slot(
            "restore_hosted",
            generation,
            Some("portal".into()),
            Some("nonce".into()),
            Some("key".into()),
        ));
        assert!(start_worker(&slot, generation));
        slot.lock().unwrap().current.as_mut().unwrap().started =
            Instant::now() - HANDOFF_TTL - Duration::from_secs(1);
        mark_cancelled(&slot, generation);
        mark_expired(&slot, generation);
        assert_eq!(observed_current(&slot).unwrap().unwrap().phase, "restoring");
        assert!(worker_or_cleanup_active(&slot, generation));
        assert!(begin(&slot, "rotate", None, None, None).is_err());
        finish_worker(
            &slot,
            generation,
            Terminal::error("original_failure"),
            Vec::new(),
        );
        let current = observed_current(&slot).unwrap().unwrap();
        assert_eq!(current.phase, "error");
        assert_eq!(current.reason_code.as_deref(), Some("original_failure"));
        let guard = slot.lock().unwrap();
        let current = guard.current.as_ref().unwrap();
        assert!(!current.worker_active);
        assert!(current.cleanup.is_none());
        assert!(
            current.nonce.is_none()
                && current.restore_key.is_none()
                && current.view.portal_url.is_none()
        );
    }

    #[test]
    fn stale_worker_and_recovery_cannot_clear_a_new_generation() {
        let slot = new_slot();
        let first = 1;
        slot.lock().unwrap().current = Some(admitted_slot("rotate", first, None, None, None));
        assert!(start_worker(&slot, first));
        finish_worker(&slot, first, Terminal::done(), Vec::new());
        let second = {
            let mut guard = slot.lock().unwrap();
            let settled = guard.current.as_ref().unwrap();
            assert_eq!(settled.view.phase, "done");
            assert!(!settled.worker_active);
            assert!(settled.cleanup.is_none());
            let generation = settled.generation.wrapping_add(1);
            guard.current = Some(admitted_slot("restore", generation, None, None, None));
            generation
        };
        assert!(start_worker(&slot, second));
        finish_worker(&slot, first, Terminal::error("stale"), Vec::new());
        assert!(observed_generation(&slot, Some(first)).is_err());
        assert!(worker_or_cleanup_active(&slot, second));
        assert_eq!(observed_current(&slot).unwrap().unwrap().phase, "restoring");
        finish_worker(&slot, second, Terminal::done(), Vec::new());
    }

    #[test]
    fn status_observation_refuses_slot_contention_without_waiting() {
        let slot = new_slot();
        let guard = slot.lock().unwrap();
        assert!(observed_current(&slot).is_err());
        assert!(observed_generation(&slot, None).is_err());
        drop(guard);
        assert!(observed_current(&slot).unwrap().is_none());
    }
}
