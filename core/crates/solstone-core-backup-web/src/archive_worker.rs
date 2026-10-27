// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Keep concrete archive owners independently of the displayed operation.
//! Actor execution consumes the original admitted read in its owning worker.

pub mod offload;
pub mod removal;
pub mod restore;
pub(crate) mod teardown;

use std::{
    io,
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Arc,
};

use serde_json::{Value, json};
use solstone_core_backup::archive_activity::{
    ArchivePreparationCustody, OwnedArchivePreparationFailure, OwnedArchivePreparationInput,
    OwnedPreparedArchiveHold,
};
use solstone_core_backup::archive_admission::{ArchiveBudget, ArchiveCancellation, Clock};
use solstone_core_backup::archive_readback::{OwnedDirectReadHold, OwnedReadOperation};
use solstone_core_backup_runtime::{
    DirectReadFinishOwner, DirectReadFinishState, OwnedArchiveAdmissionFailure,
    OwnedDirectReadFinish, UreqHttpTransport, admit_prepared_archive_owned_current_read_installed,
    admit_prepared_archive_owned_read_installed,
};
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::journal_root::JournalRoot;
use solstone_core_offload::restore::owned::{OwnedRestoreFinishHold, SettledRestoreReport};
use solstone_core_retention::remove_marked::recover_interrupted_removals;
use solstone_core_retention::remove_marked::{
    OwnedReviewedArchiveRemoval, OwnedReviewedRemoval, ReviewedArchiveReport, ReviewedRemovalReport,
};
use solstone_core_retention::restore_intent::recover_abandoned_restores;

use crate::archive_operation::ArchiveBeginRefusal;
use crate::operation::{self, SharedOperationSlot, Terminal};

type OwnedClock = Arc<dyn Clock + Send + Sync>;

struct Reservation {
    generation: u64,
    root: Arc<JournalRoot>,
    clock: OwnedClock,
}

/// Only the original admission can return its reservation.
#[must_use]
pub struct ArchiveTicket(Arc<Reservation>);

impl ArchiveTicket {
    pub fn generation(&self) -> u64 {
        self.0.generation
    }
}

/// Original budget and clock, before any native preparation or transport.
#[must_use]
pub struct OwnedArchiveBegin {
    ticket: ArchiveTicket,
    budget: ArchiveBudget,
}

impl OwnedArchiveBegin {
    pub fn generation(&self) -> u64 {
        self.ticket.generation()
    }

    pub fn into_parts(self) -> (ArchiveTicket, Arc<JournalRoot>, ArchiveBudget, OwnedClock) {
        let root = Arc::clone(&self.ticket.0.root);
        let clock = Arc::clone(&self.ticket.0.clock);
        (self.ticket, root, self.budget, clock)
    }

    /// Consume only an admission that has not issued its preparation inputs.
    pub fn cancel_before_preparation(self, slot: &SharedOperationSlot) {
        if let Some(flag) = self.budget.cancellation() {
            flag.cancel();
        }
        let Ok(mut state) = slot.lock() else {
            return;
        };
        if !state.archive.matches(&self.ticket) {
            return;
        }
        if let Some(current) = state.current.as_mut()
            && current.generation == self.generation()
            && !current.worker_active
        {
            operation::apply_terminal(current, Terminal::error("cancelled"));
        }
        state.archive.reservation = None;
    }
}

/// Reserve the same shared mutex before creating the original budget.
pub fn begin_owned(
    slot: &SharedOperationSlot,
    kind: &str,
    root: Arc<JournalRoot>,
    clock: OwnedClock,
) -> Result<OwnedArchiveBegin, ArchiveBeginRefusal> {
    let mut state = operation::lock_for_begin(slot).map_err(|()| ArchiveBeginRefusal::Busy)?;
    begin_owned_locked(&mut state, kind, root, clock)
}

fn begin_owned_locked(
    state: &mut operation::OperationState,
    kind: &str,
    root: Arc<JournalRoot>,
    clock: OwnedClock,
) -> Result<OwnedArchiveBegin, ArchiveBeginRefusal> {
    begin_owned_locked_for_task(state, kind, root, clock, None)
}

/// Watch one queue task from this operation's child, without cancelling the service.
pub(crate) fn begin_owned_for_task(
    slot: &SharedOperationSlot,
    kind: &str,
    root: Arc<JournalRoot>,
    clock: OwnedClock,
    task: Arc<dyn Fn() -> Result<(), String> + Send + Sync>,
) -> Result<OwnedArchiveBegin, ArchiveBeginRefusal> {
    task().map_err(|_| {
        ArchiveBeginRefusal::Admission(
            solstone_core_backup::archive_admission::AdmissionError::Cancelled,
        )
    })?;
    let mut state = operation::lock_for_begin(slot).map_err(|()| ArchiveBeginRefusal::Busy)?;
    begin_owned_locked_for_task(&mut state, kind, root, clock, Some(task))
}

fn begin_owned_locked_for_task(
    state: &mut operation::OperationState,
    kind: &str,
    root: Arc<JournalRoot>,
    clock: OwnedClock,
    task: Option<Arc<dyn Fn() -> Result<(), String> + Send + Sync>>,
) -> Result<OwnedArchiveBegin, ArchiveBeginRefusal> {
    let child = ArchiveCancellation::new_child(&state.cancellation);
    let cancellation = match task {
        Some(task) => child.with_task(task),
        None => child,
    };
    let budget = ArchiveBudget::new_cancellable(clock.as_ref(), cancellation.clone())
        .map_err(ArchiveBeginRefusal::Admission)?;
    let generation = operation::install_slot(
        &mut state.current,
        kind,
        None,
        None,
        None,
        Some(cancellation),
    )
    .generation;
    let reservation = Arc::new(Reservation {
        generation,
        root,
        clock,
    });
    state.archive.reservation = Some(Arc::clone(&reservation));
    Ok(OwnedArchiveBegin {
        ticket: ArchiveTicket(reservation),
        budget,
    })
}

/// Closed concrete values only; no report can construct a native owner.
#[must_use]
pub enum ArchivePayload {
    Offload(Box<offload::OffloadReturn>),
    BeginHeld(Box<OwnedArchiveBegin>),
    EarlyUnknown,
    NativeFinishUnknown {
        report: SettledRestoreReport,
    },
    ProofFinishUnknown {
        report: ReviewedArchiveReport,
    },
    Prepared(Box<OwnedPreparedArchiveHold>),
    PreparationFailure(Box<OwnedArchivePreparationFailure>),
    UnusedRetirement(Box<solstone_core_backup::archive_activity::OwnedUnusedArchiveRetirement>),
    AdmissionFailure(OwnedArchiveAdmissionFailure),
    ReadHeld {
        read: Box<OwnedDirectReadHold>,
        outside: DirectReadFinishOwner,
    },
    NativeHeld {
        native: Box<OwnedRestoreFinishHold>,
        outside: DirectReadFinishOwner,
    },
    NativeFinished {
        report: SettledRestoreReport,
        runtime: OwnedDirectReadFinish,
    },
    ProofHeld {
        proof: Box<OwnedReviewedArchiveRemoval>,
        outside: DirectReadFinishOwner,
    },
    ProofFinished {
        report: ReviewedArchiveReport,
        runtime: OwnedDirectReadFinish,
    },
    ProofUnknown {
        runtime: OwnedDirectReadFinish,
    },
    Unknown {
        runtime: OwnedDirectReadFinish,
        report: Option<SettledRestoreReport>,
    },
    /// Every owner ended with nothing outstanding. Never retained.
    Released,
    /// A local policy release keeping its original activity and effects.
    OrdinaryHeld(Box<OwnedReviewedRemoval>),
    /// A local policy release whose own activity carrier was retired.
    OrdinarySettled(Box<ReviewedRemovalReport>),
    /// A local policy release that unwound; its effects are not known.
    OrdinaryUnknown,
}

impl ArchivePayload {
    fn completed(&self) -> bool {
        if let Self::Offload(owner) = self {
            return owner
                .runtime()
                .is_some_and(|runtime| runtime.state() == DirectReadFinishState::Completed);
        }
        matches!(self, Self::OrdinarySettled(_))
            || matches!(self, Self::NativeFinished { runtime, .. } | Self::ProofFinished { runtime, .. }
            if runtime.state() == DirectReadFinishState::Completed)
    }

    /// End every owner of a return that provably leaves nothing outstanding: no
    /// live child, no stage of its own, nothing published or pending. Anything
    /// uncertain comes back whole. This never reports success.
    fn release(self) -> Result<(), Self> {
        match self {
            Self::Released | Self::BeginHeld(_) => Ok(()),
            Self::Prepared(owner) => owner
                .retire_unused()
                .map_err(|retirement| Self::UnusedRetirement(Box::new(retirement))),
            Self::PreparationFailure(failure) => failure
                .retire_unused()
                .map_err(|retirement| Self::UnusedRetirement(Box::new(retirement))),
            Self::AdmissionFailure(failure) => failure
                .release()
                .map_err(|failure| Self::AdmissionFailure(*failure)),
            Self::ReadHeld { read, outside } => match (*read).release_refused() {
                Ok(read) => {
                    let runtime = outside.finish_refused(read);
                    if runtime.released() {
                        Ok(())
                    } else {
                        Err(Self::Unknown {
                            runtime,
                            report: None,
                        })
                    }
                }
                Err(read) => Err(Self::ReadHeld { read, outside }),
            },
            Self::NativeHeld { native, outside } => match (*native).release_refused() {
                Ok((read, report)) => {
                    let runtime = outside.finish_refused(read);
                    if runtime.released() {
                        Ok(())
                    } else {
                        Err(Self::NativeFinished { report, runtime })
                    }
                }
                Err(native) => Err(Self::NativeHeld { native, outside }),
            },
            Self::ProofHeld { proof, outside } => match (*proof).release_refused() {
                Ok((read, report)) => {
                    let runtime = outside.finish_refused(read);
                    if runtime.released() {
                        Ok(())
                    } else {
                        Err(Self::ProofFinished { report, runtime })
                    }
                }
                Err(proof) => Err(Self::ProofHeld { proof, outside }),
            },
            Self::NativeFinished { runtime, .. } | Self::ProofFinished { runtime, .. }
                if runtime.released() =>
            {
                Ok(())
            }
            Self::Offload(owner) => (*owner).release().map_err(Self::Offload),
            // A settled local release completes through handoff, which frees
            // the slot once and records its success; a held or unknown local
            // release keeps its owner parked.
            local @ (Self::OrdinarySettled(_) | Self::OrdinaryHeld(_) | Self::OrdinaryUnknown) => {
                Err(local)
            }
            other => Err(other),
        }
    }

    fn matches(
        &self,
        reservation: &Reservation,
        custody: Option<&ArchivePreparationCustody>,
    ) -> bool {
        if custody.is_some_and(|owner| {
            !Arc::ptr_eq(owner.journal_root(), &reservation.root)
                || !Arc::ptr_eq(owner.owned_clock(), &reservation.clock)
        }) {
            return false;
        }
        let (root, clock) = match self {
            Self::Released => return true,
            Self::Offload(owner) => return owner.matches(reservation, custody),
            // Local releases never take archive preparation custody.
            Self::OrdinaryHeld(owner) => {
                return custody.is_none()
                    && std::ptr::eq(owner.removal().journal_root(), &*reservation.root);
            }
            Self::OrdinarySettled(_) | Self::OrdinaryUnknown => return custody.is_none(),
            Self::BeginHeld(owner) => (&owner.ticket.0.root, Some(&owner.ticket.0.clock)),
            Self::EarlyUnknown
            | Self::NativeFinishUnknown { .. }
            | Self::ProofFinishUnknown { .. } => {
                let Some(owner) = custody else {
                    return false;
                };
                (owner.journal_root(), Some(owner.owned_clock()))
            }
            Self::Prepared(owner) => (owner.journal_root(), Some(owner.owned_clock())),
            Self::PreparationFailure(owner) => (owner.journal_root(), Some(owner.owned_clock())),
            Self::UnusedRetirement(owner) => (owner.journal_root(), Some(owner.owned_clock())),
            Self::AdmissionFailure(owner) => (owner.journal_root(), owner.owned_clock()),
            Self::ReadHeld { read, outside } => {
                if !Arc::ptr_eq(read.carrier(), outside.carrier())
                    || !Arc::ptr_eq(read.journal_root(), outside.journal_root())
                    || !Arc::ptr_eq(read.owned_clock(), outside.owned_clock())
                {
                    return false;
                }
                (outside.journal_root(), Some(outside.owned_clock()))
            }
            Self::NativeHeld { native, outside } => {
                if !Arc::ptr_eq(native.carrier(), outside.carrier()) {
                    return false;
                }
                (outside.journal_root(), Some(outside.owned_clock()))
            }
            Self::ProofHeld { proof, outside } => {
                if !Arc::ptr_eq(proof.archive_carrier(), outside.carrier()) {
                    return false;
                }
                (outside.journal_root(), Some(outside.owned_clock()))
            }
            Self::NativeFinished { runtime, .. }
            | Self::Unknown { runtime, .. }
            | Self::ProofFinished { runtime, .. }
            | Self::ProofUnknown { runtime } => {
                (runtime.journal_root(), Some(runtime.owned_clock()))
            }
        };
        Arc::ptr_eq(root, &reservation.root)
            && clock.is_some_and(|clock| Arc::ptr_eq(clock, &reservation.clock))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArchiveHandoffRefusal {
    Poisoned,
    Missing,
    Stale,
    Inactive,
    ReservationChanged,
    OwnerChanged,
    NonterminalCompletion,
    CustodyMissing,
    BindingChanged,
}

/// A rejected handoff still owns the exact ticket, payload and effect projection.
#[must_use = "park this whole refusal before the worker returns"]
pub struct RejectedArchiveReturn {
    ticket: ArchiveTicket,
    payload: ArchivePayload,
    custody: Option<ArchivePreparationCustody>,
    binding_error: Option<io::Error>,
    terminal: Terminal,
    reason: Option<ArchiveHandoffRefusal>,
}

impl RejectedArchiveReturn {
    pub fn reason(&self) -> Option<ArchiveHandoffRefusal> {
        self.reason
    }
    pub fn generation(&self) -> u64 {
        self.ticket.generation()
    }
    pub fn payload(&self) -> &ArchivePayload {
        &self.payload
    }
    pub fn custody(&self) -> Option<&ArchivePreparationCustody> {
        self.custody.as_ref()
    }
    pub fn binding_error(&self) -> Option<&io::Error> {
        self.binding_error.as_ref()
    }
    pub fn terminal(&self) -> &Terminal {
        &self.terminal
    }
}

#[derive(Default)]
pub(crate) struct ArchiveCustody {
    reservation: Option<Arc<Reservation>>,
    // Never replace a retained return, even after a rejected/stale handoff.
    retained: Vec<RejectedArchiveReturn>,
    teardown: teardown::Custody,
    approval: removal::ApprovalCustody,
    restore: restore::Custody,
}

impl ArchiveCustody {
    pub(crate) fn needs_settlement(&self) -> bool {
        !self.retained.is_empty()
            || self.teardown.needs_settlement()
            || self.approval.needs_settlement()
            || self.restore.needs_settlement()
    }
    pub(crate) fn workers_running(&self) -> bool {
        !self.approval.settled() || self.restore.worker_running()
    }
    pub(crate) fn is_busy(&self) -> bool {
        self.reservation.is_some()
            || !self.retained.is_empty()
            || self.teardown.is_busy()
            || self.approval.is_busy()
            || self.restore.is_busy()
    }

    pub(crate) fn retained(&self) -> impl Iterator<Item = &RejectedArchiveReturn> {
        self.retained.iter()
    }

    fn matches(&self, ticket: &ArchiveTicket) -> bool {
        self.reservation
            .as_ref()
            .is_some_and(|original| Arc::ptr_eq(original, &ticket.0))
    }

    pub(crate) fn take_finished_workers(&mut self) -> Vec<std::thread::JoinHandle<()>> {
        let mut workers = Vec::new();
        if let Some(worker) = self.approval.take_finished_worker() {
            workers.push(worker);
        }
        if let Some(worker) = self.restore.take_finished_worker() {
            workers.push(worker);
        }
        workers
    }
}

/// Start only the worker owning this private admission ticket.
pub fn start_owned_worker(slot: &SharedOperationSlot, ticket: &ArchiveTicket) -> bool {
    let Ok(mut state) = slot.lock() else {
        return false;
    };
    if state.closed || !state.archive.matches(ticket) || !state.archive.retained.is_empty() {
        return false;
    }
    let Some(current) = state.current.as_mut() else {
        return false;
    };
    if current.generation != ticket.generation()
        || current.worker_active
        || operation::is_terminal(&current.view.phase)
        || current
            .archive_cancellation
            .as_ref()
            .is_some_and(|flag| flag.is_cancelled())
        || current.pending_handoff_terminal.is_some()
        || current.handoff_poll.is_some()
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

/// Return complete original custody; matching completion also needs fresh binding.
pub fn handoff(
    slot: &SharedOperationSlot,
    ticket: ArchiveTicket,
    custody: Option<ArchivePreparationCustody>,
    payload: ArchivePayload,
    terminal: Terminal,
) -> Result<(), Box<RejectedArchiveReturn>> {
    let mut returned = Box::new(RejectedArchiveReturn {
        ticket,
        payload,
        custody,
        binding_error: None,
        terminal,
        reason: None,
    });
    let Ok(mut state) = slot.lock() else {
        returned.reason = Some(ArchiveHandoffRefusal::Poisoned);
        return Err(returned);
    };
    let reason = if !state.archive.matches(&returned.ticket) || !state.archive.retained.is_empty() {
        Some(ArchiveHandoffRefusal::ReservationChanged)
    } else if !returned
        .payload
        .matches(&returned.ticket.0, returned.custody.as_ref())
    {
        Some(ArchiveHandoffRefusal::OwnerChanged)
    } else {
        match &state.current {
            None => Some(ArchiveHandoffRefusal::Missing),
            Some(current) if current.generation != returned.ticket.generation() => {
                Some(ArchiveHandoffRefusal::Stale)
            }
            Some(current)
                if !current.worker_active
                    || operation::is_terminal(&current.view.phase)
                    || current.pending_handoff_terminal.is_some()
                    || current.handoff_poll.is_some() =>
            {
                Some(ArchiveHandoffRefusal::Inactive)
            }
            Some(_)
                if returned.payload.completed()
                    && !operation::is_terminal(&returned.terminal.phase) =>
            {
                Some(ArchiveHandoffRefusal::NonterminalCompletion)
            }
            Some(_) => None,
        }
    };
    if matches!(returned.payload, ArchivePayload::Released) {
        discharge_released(&mut state, &returned);
        return Ok(());
    }
    if let Some(reason) = reason {
        returned.reason = Some(reason);
        return Err(returned);
    }
    if returned.payload.completed() {
        // A settled local release confirmed retirement of its own carrier
        // before returning; only archive owners carry runtime custody.
        if !matches!(returned.payload, ArchivePayload::OrdinarySettled(_)) {
            let Some(custody) = returned.custody.as_ref() else {
                returned.reason = Some(ArchiveHandoffRefusal::CustodyMissing);
                return Err(returned);
            };
            let runtime = match &returned.payload {
                ArchivePayload::Offload(owner) => {
                    owner.runtime().expect("actual completed offload runtime")
                }
                ArchivePayload::NativeFinished { runtime, .. }
                | ArchivePayload::ProofFinished { runtime, .. } => runtime,
                _ => unreachable!("only actual finished owners can complete"),
            };
            if !runtime.retirement().is_some_and(|retirement| {
                std::ptr::eq(custody.activity(), retirement.owned.activity().as_ref())
            }) {
                returned.reason = Some(ArchiveHandoffRefusal::OwnerChanged);
                return Err(returned);
            }
            if let Err(error) = custody.revalidate() {
                returned.reason = Some(ArchiveHandoffRefusal::BindingChanged);
                returned.binding_error = Some(error);
                return Err(returned);
            }
        }
        let current = state.current.as_mut().expect("checked current");
        current.worker_active = false;
        operation::apply_terminal(current, returned.terminal.clone());
        if state.archive.approval.generation == Some(returned.ticket.generation()) {
            state.archive.approval.accepted_completion = true;
        }
        // All actual owners, including original lease custody, release while
        // this same admission mutex excludes a successor.
        drop(returned);
        state.archive.reservation = None;
    } else {
        let current = state.current.as_mut().expect("checked current");
        current.worker_active = false;
        let phase = current.view.phase.clone();
        operation::apply_terminal(current, returned.terminal.clone());
        current.view.phase = phase;
        state.archive.retained.push(*returned);
    }
    Ok(())
}

/// A released return owns nothing, so it is never retained. Its own reservation
/// ends; a still-current view takes its refused or failed terminal.
fn discharge_released(state: &mut operation::OperationState, returned: &RejectedArchiveReturn) {
    if !state.archive.matches(&returned.ticket) {
        return;
    }
    state.archive.reservation = None;
    if let Some(current) = state.current.as_mut()
        && current.generation == returned.generation()
        && current.handoff_poll.is_none()
        && current.pending_handoff_terminal.is_none()
        && (current.worker_active || !operation::is_terminal(&current.view.phase))
    {
        current.worker_active = false;
        operation::apply_terminal(current, returned.terminal.clone());
        if state.archive.approval.generation == Some(returned.generation()) {
            state.archive.approval.discharge(&returned.terminal);
        }
    }
}

/// Retry the same valid return after its native helper finishes stopping.
/// Release still requires the owner's proof that no effects remain; rejected
/// handoffs and uncertain effects keep their original custody.
#[cfg(windows)]
pub(crate) fn observe_retained(state: &mut operation::OperationState) {
    for mut returned in std::mem::take(&mut state.archive.retained) {
        if returned.reason.is_none() && state.archive.matches(&returned.ticket) {
            let payload = std::mem::replace(&mut returned.payload, ArchivePayload::Released);
            match payload.release() {
                Ok(()) => {
                    discharge_released(state, &returned);
                    continue;
                }
                Err(payload) => returned.payload = payload,
            }
        }
        state.archive.retained.push(returned);
    }
}

/// A closed lifetime's actual settlement worker. Custody stays in the shared
/// state until this registered worker takes it, then release runs outside its mutex.
pub(crate) fn settle_closed(slot: &SharedOperationSlot) {
    let returned = {
        let mut state = match slot.lock() {
            Ok(state) => state,
            Err(_) => return,
        };
        if !state.closed {
            return;
        }
        let mut eligible = Vec::new();
        for returned in std::mem::take(&mut state.archive.retained) {
            if returned.reason.is_none() && state.archive.matches(&returned.ticket) {
                eligible.push(returned);
            } else {
                state.archive.retained.push(returned);
            }
        }
        eligible
    };
    for mut returned in returned {
        let payload = std::mem::replace(&mut returned.payload, ArchivePayload::Released);
        let released = match payload.release() {
            Ok(()) => true,
            Err(payload) => {
                returned.payload = payload;
                false
            }
        };
        let mut state = match slot.lock() {
            Ok(state) => state,
            Err(error) => {
                error.into_inner().archive.retained.push(returned);
                continue;
            }
        };
        if released && state.archive.matches(&returned.ticket) {
            discharge_released(&mut state, &returned);
        } else {
            state.archive.retained.push(returned);
        }
    }
    let mut state = match slot.lock() {
        Ok(state) => state,
        Err(_) => return,
    };
    #[cfg(windows)]
    if let Some(current) = state.current.as_mut() {
        operation::observe_cleanup(current);
    }
    teardown::settle_closed(&mut state);
    removal::settle_closed(&mut state);
    restore::settle_closed(&mut state);
    slot.notify_waiters();
}

/// Retention alone may recover a poisoned mutex. It never applies success.
pub fn park_rejected(slot: &SharedOperationSlot, rejected: Box<RejectedArchiveReturn>) {
    let (mut state, poisoned) = match slot.lock() {
        Ok(state) => (state, false),
        Err(poison) => (poison.into_inner(), true),
    };
    if matches!(rejected.payload, ArchivePayload::Released) {
        if !poisoned {
            discharge_released(&mut state, &rejected);
        } else if state.archive.matches(&rejected.ticket) {
            state.archive.reservation = None;
        }
        return;
    }
    if !poisoned
        && rejected.reason == Some(ArchiveHandoffRefusal::BindingChanged)
        && state.archive.matches(&rejected.ticket)
        && let Some(current) = state.current.as_mut()
        && current.generation == rejected.generation()
        && current.worker_active
        && !operation::is_terminal(&current.view.phase)
        && current.handoff_poll.is_none()
        && current.pending_handoff_terminal.is_none()
    {
        #[cfg(windows)]
        let can_project = current.cleanup.is_none();
        #[cfg(not(windows))]
        let can_project = true;
        if can_project {
            let mut visible = rejected.terminal.clone();
            visible.phase = "error".into();
            visible.reason_code = Some("failed".into());
            current.worker_active = false;
            operation::apply_terminal(current, visible);
        }
    }
    state.archive.retained.push(*rejected);
}

pub(super) fn entry(
    slot: &SharedOperationSlot,
    ticket: &ArchiveTicket,
    read: &OwnedReadOperation,
    outside: &DirectReadFinishOwner,
) -> Result<(), &'static str> {
    if !Arc::ptr_eq(read.journal_root(), &ticket.0.root)
        || !Arc::ptr_eq(read.owned_clock(), &ticket.0.clock)
        || !Arc::ptr_eq(read.carrier(), outside.carrier())
        || !Arc::ptr_eq(read.journal_root(), outside.journal_root())
        || !Arc::ptr_eq(read.owned_clock(), outside.owned_clock())
    {
        return Err("failed");
    }
    entry_current(slot, ticket)
}

pub(super) fn entry_current(
    slot: &SharedOperationSlot,
    ticket: &ArchiveTicket,
) -> Result<(), &'static str> {
    let state = slot.lock().map_err(|_| "failed")?;
    if !state.archive.matches(ticket) || !state.archive.retained.is_empty() {
        return Err("failed");
    }
    let current = state.current.as_ref().ok_or("failed")?;
    if current.generation != ticket.generation()
        || !current.worker_active
        || operation::is_terminal(&current.view.phase)
        || current.pending_handoff_terminal.is_some()
        || current.handoff_poll.is_some()
    {
        return Err("failed");
    }
    #[cfg(windows)]
    if current.cleanup.is_some() {
        return Err("failed");
    }
    if current
        .archive_cancellation
        .as_ref()
        .is_some_and(|flag| flag.is_cancelled())
    {
        return Err("cancelled");
    }
    Ok(())
}

pub(super) fn runtime_facts(runtime: &OwnedDirectReadFinish) -> Value {
    use solstone_core_journal_io::media::ActivityCarrierRetirementEffect as Effect;
    json!({
        "state":match runtime.state() {
            DirectReadFinishState::Completed => "completed",
            DirectReadFinishState::Held => "held",
            DirectReadFinishState::Unknown => "unknown",
        },
        "reason":runtime.reason().map(|reason| reason.to_string()),
        "retirement":runtime.retirement().map(|retirement| json!({
            "effect":match retirement.receipt.effect() {
                Effect::NotAttempted => "not_attempted",
                Effect::Attempted => "attempted",
                Effect::Removed => "removed",
            },
            "root_durability":retirement.receipt.root_durability(),
            "confirmation":retirement.receipt.confirmation(),
            "failures":retirement.receipt.failures().iter().map(|failure| json!({
                "error":failure.kind().to_string(),
            })).collect::<Vec<_>>(),
        })),
    })
}

pub(super) fn return_owned(
    slot: &SharedOperationSlot,
    ticket: ArchiveTicket,
    custody: Option<ArchivePreparationCustody>,
    payload: ArchivePayload,
    mut terminal: Terminal,
) {
    // Release while the original lease custody still excludes a successor.
    let payload = match payload.release() {
        Ok(()) => {
            if terminal.phase == "done" {
                terminal.phase = "error".into();
                terminal.reason_code = Some("failed".into());
            }
            ArchivePayload::Released
        }
        Err(payload) => payload,
    };
    if let Err(rejected) = handoff(slot, ticket, custody, payload, terminal) {
        park_rejected(slot, rejected);
    }
}

/// Original preparation custody and installed read pair, kept worker-local.
#[must_use]
pub struct PreparedArchiveWorker {
    ticket: ArchiveTicket,
    custody: ArchivePreparationCustody,
    read: OwnedReadOperation,
    outside: DirectReadFinishOwner,
}

/// Consume one original admission; every refusal is parked before returning.
pub fn prepare_read<'a>(
    slot: &SharedOperationSlot,
    begin: OwnedArchiveBegin,
    repository_id: String,
    restic: impl Into<solstone_core_backup_runtime::ResticInvocation<'a>>,
) -> Option<PreparedArchiveWorker> {
    prepare_read_in(slot, begin, Some(repository_id), restic)
}

/// Without an expected identity, the read binds to the configured repository
/// as observed at admission.
pub(crate) fn prepare_read_in<'a>(
    slot: &SharedOperationSlot,
    begin: OwnedArchiveBegin,
    repository_id: Option<String>,
    restic: impl Into<solstone_core_backup_runtime::ResticInvocation<'a>>,
) -> Option<PreparedArchiveWorker> {
    let restic = restic.into();
    if !start_owned_worker(slot, &begin.ticket) {
        let ticket = ArchiveTicket(Arc::clone(&begin.ticket.0));
        return_owned(
            slot,
            ticket,
            None,
            ArchivePayload::BeginHeld(Box::new(begin)),
            Terminal::error("failed"),
        );
        return None;
    }
    // A removal or restore interrupted by a crash would refuse this admission.
    // An unqualified phone container refuses before any archive work.
    if let Ok(activity) = ActivityLease::try_acquire(Arc::clone(&begin.ticket.0.root))
        && let Err(_error) = recover_interrupted_removals(&activity)
            .and_then(|_| recover_abandoned_restores(&activity).map(|_| ()))
    {
        let mut terminal = Terminal::error("failed");
        terminal.report = Some(json!({
            "preparation": {"state": "refused", "reason": "recovery_unavailable"}
        }));
        let ticket = ArchiveTicket(Arc::clone(&begin.ticket.0));
        return_owned(
            slot,
            ticket,
            None,
            ArchivePayload::BeginHeld(Box::new(begin)),
            terminal,
        );
        return None;
    }
    let (ticket, root, budget, clock) = begin.into_parts();
    let (input, custody) = match OwnedArchivePreparationInput::capture(root, budget, clock) {
        Ok(pair) => pair,
        Err(failure) => {
            let mut terminal = Terminal::error("failed");
            terminal.report = Some(
                json!({"preparation":{"state":"refused","reason":failure.reason().to_string()}}),
            );
            return_owned(
                slot,
                ticket,
                None,
                ArchivePayload::PreparationFailure(Box::new(failure)),
                terminal,
            );
            return None;
        }
    };
    let mut terminal = Terminal::error("failed");
    let mut phase = "preparation";
    let admitted = catch_unwind(AssertUnwindSafe(|| {
        let prepared = input.prepare().map_err(|failure| {
            terminal.report = Some(
                json!({"preparation":{"state":"refused","reason":failure.reason().to_string()}}),
            );
            ArchivePayload::PreparationFailure(Box::new(failure))
        })?;
        phase = "admission";
        match repository_id {
            Some(repository_id) => admit_prepared_archive_owned_read_installed(
                prepared,
                repository_id,
                restic,
                env!("CARGO_PKG_VERSION"),
                &UreqHttpTransport,
            ),
            None => admit_prepared_archive_owned_current_read_installed(
                prepared,
                restic,
                env!("CARGO_PKG_VERSION"),
                &UreqHttpTransport,
            ),
        }
        .map_err(|failure| {
            terminal.report = Some(
                json!({"admission":{"state":"refused","reason":failure.error().to_string(),
                    "broker_requested":failure.effects().broker_requested,
                    "write_repository_requested":failure.effects().write_repository_requested,
                    "read_repository_requested":failure.effects().read_repository_requested}}),
            );
            ArchivePayload::AdmissionFailure(failure)
        })
    }));
    let payload = match admitted {
        Ok(Ok(installed)) => {
            let (read, outside) = installed.into_parts();
            return Some(PreparedArchiveWorker {
                ticket,
                custody,
                read,
                outside,
            });
        }
        Ok(Err(payload)) => payload,
        Err(_) => {
            terminal.report = Some(json!({phase:{"state":"unknown"}}));
            ArchivePayload::EarlyUnknown
        }
    };
    return_owned(slot, ticket, Some(custody), payload, terminal);
    None
}

fn retained_archive_root(
    deps: &crate::BackupWebDeps,
) -> Result<Arc<JournalRoot>, solstone_core_journal_io::JournalRootError> {
    let root = deps.retained_root()?;
    #[cfg(target_os = "ios")]
    {
        let runner_root = deps
            .runner
            .ios_journal_root()
            .ok_or(solstone_core_journal_io::JournalRootError::Changed)?;
        if !Arc::ptr_eq(&root, runner_root) {
            return Err(solstone_core_journal_io::JournalRootError::Changed);
        }
    }
    Ok(root)
}

fn resolve_read_invocation(
    root: &Arc<JournalRoot>,
    executable: &Path,
) -> Result<
    solstone_core_backup_runtime::ResolvedResticInvocation,
    solstone_core_backup_runtime::ClosedToolError,
> {
    #[cfg(target_os = "ios")]
    {
        let _ = executable;
        solstone_core_backup_runtime::ios_callable::admitted_ios_backup(Arc::clone(root))
            .map(solstone_core_backup_runtime::ResolvedResticInvocation::Callable)
    }
    #[cfg(not(target_os = "ios"))]
    {
        let _ = root;
        solstone_core_backup_runtime::resolve_restic_member(executable)
            .map(solstone_core_backup_runtime::ResolvedResticInvocation::Executable)
    }
}

#[cfg(test)]
mod type_tests {
    use super::*;
    #[test]
    fn closed_archive_custody_is_send_static() {
        fn check<T: Send + 'static>() {}
        check::<ArchivePayload>();
        check::<RejectedArchiveReturn>();
        check::<OwnedArchiveBegin>();
        check::<ArchivePreparationCustody>();
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(unix)]
mod tests;
