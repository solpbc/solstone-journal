// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Run an already admitted reviewed removal and retain its whole owning return.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::PathBuf,
    sync::Arc,
    thread::JoinHandle,
};

use axum::{
    Json,
    response::{IntoResponse, Response},
};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Serialize;
use serde_json::{Value, json};
use solstone_core_backup::archive_admission::AdmissionError;
use solstone_core_backup::archive_readback::{OwnedSettledDirectRead, RefusedDirectRead};
use solstone_core_backup_runtime::ClosedToolError;
use solstone_core_journal_io::JournalRoot;
use solstone_core_retention::Outcome;
use solstone_core_retention::marks::RemovalClass;
use solstone_core_retention::remove_marked::{
    OwnedReviewedArchiveFinish, ReviewedArchiveReport, ReviewedRemovalFinish, remove_reviewed,
    remove_reviewed_with_owned_archive,
};
use solstone_core_retention::review_context::{
    CheckedReviewContext, ReviewContext, ReviewRefusal, check_review_context,
};

use super::{
    ArchivePayload, ArchiveTicket, OwnedArchiveBegin, PreparedArchiveWorker, begin_owned_locked,
    entry, entry_current, prepare_read, return_owned, runtime_facts, start_owned_worker,
};
use crate::operation::{self, OperationState, SharedOperationSlot, Terminal};
use crate::removal_report;
use crate::{BackupWebDeps, archive_operation::ArchiveBeginRefusal};

const KIND: &str = "offload_approve";

struct ApprovalInput {
    begin: OwnedArchiveBegin,
    checked: CheckedReviewContext,
    /// Absent only when no archive offer is selected: a local policy release.
    repository: Option<String>,
    executable: PathBuf,
    today: NaiveDate,
    now: DateTime<Utc>,
}

enum UnusedRefusal {
    Start,
    Budget(AdmissionError),
    Tools(ClosedToolError),
    ResolverUnknown,
}

struct UnusedApproval {
    input: ApprovalInput,
    refusal: UnusedRefusal,
}

#[derive(Default)]
pub(super) struct ApprovalCustody {
    pub(super) generation: Option<u64>,
    pub(super) accepted_completion: bool,
    ticket: Option<ArchiveTicket>,
    discharged_refusal: Option<String>,
    input: Option<ApprovalInput>,
    unused: Vec<UnusedApproval>,
    refused_selection: Option<CheckedReviewContext>,
    worker: Option<JoinHandle<()>>,
}

impl ApprovalCustody {
    pub(super) fn needs_settlement(&self) -> bool {
        !self.unused.is_empty()
    }
    pub(super) fn is_busy(&self) -> bool {
        self.input.is_some()
            || !self.unused.is_empty()
            || self.refused_selection.is_some()
            || self
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
    }

    pub(super) fn settled(&self) -> bool {
        self.worker.as_ref().is_none_or(JoinHandle::is_finished)
    }

    pub(super) fn take_finished_worker(&mut self) -> Option<JoinHandle<()>> {
        if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
            self.worker.take()
        } else {
            None
        }
    }

    /// Record a refused approval whose whole return was released.
    pub(super) fn discharge(&mut self, terminal: &Terminal) {
        self.refused_selection = None;
        self.discharged_refusal = Some(
            terminal
                .reason_code
                .clone()
                .unwrap_or_else(|| "failed".into()),
        );
    }
}

pub(super) fn settle_closed(state: &mut OperationState) {
    if !state.closed || !state.archive.approval.settled() {
        return;
    }
    let approval = &state.archive.approval;
    if approval.unused.len() != 1
        || !current_matches(state, &approval.unused[0].input.begin.ticket)
        || state
            .current
            .as_ref()
            .is_some_and(|current| current.worker_active)
    {
        return;
    }
    let reason = unused_reason(&approval.unused[0].refusal);
    operation::apply_terminal(
        state.current.as_mut().expect("matching current"),
        Terminal::error(reason.clone()),
    );
    state.archive.approval.discharged_refusal = Some(reason);
    drop(state.archive.approval.unused.pop());
    state.archive.reservation = None;
}

#[derive(Debug)]
pub enum ApprovalStartRefusal {
    Review(ReviewRefusal),
    UnsupportedSelection,
    Begin(ArchiveBeginRefusal),
}

/// Generation transport only. It grants no consent or release authority.
#[derive(Debug, Serialize)]
pub struct ApprovalStart {
    generation: String,
}

impl ApprovalStart {
    pub fn generation(&self) -> &str {
        &self.generation
    }
}

/// Admit a reviewed selection into the retained approval slot. Selections with
/// archive offers verify them against the backup; policy-only ones run locally.
pub fn start(
    deps: &BackupWebDeps,
    context: ReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> Result<ApprovalStart, ApprovalStartRefusal> {
    let root: Arc<JournalRoot> = super::retained_archive_root(deps)
        .map_err(|error| ApprovalStartRefusal::Review(ReviewRefusal::Root(error)))?;
    let checked =
        check_review_context(Arc::clone(&root), &context).map_err(ApprovalStartRefusal::Review)?;
    // An offload mark without its offer has no backup to verify against.
    if checked
        .marks()
        .iter()
        .any(|mark| mark.class == RemovalClass::OffloadRawRelease && mark.offload.is_none())
    {
        return Err(ApprovalStartRefusal::UnsupportedSelection);
    }
    let repository = checked.marks().iter().find_map(|mark| {
        mark.offload
            .as_ref()
            .map(|offer| offer.proof.repository_id.clone())
    });
    // Proof rechecks the entire selection and every repository before release.
    let mut state = operation::lock_for_begin(&deps.operations)
        .map_err(|()| ApprovalStartRefusal::Begin(ArchiveBeginRefusal::Busy))?;
    if state.closed {
        return Err(ApprovalStartRefusal::Begin(ArchiveBeginRefusal::Busy));
    }
    let begin = begin_owned_locked(&mut state, KIND, root, Arc::clone(&deps.clock))
        .map_err(ApprovalStartRefusal::Begin)?;
    let generation = begin.generation();
    let ticket = ArchiveTicket(Arc::clone(&begin.ticket.0));
    state.archive.approval = ApprovalCustody {
        generation: Some(generation),
        ticket: Some(ArchiveTicket(Arc::clone(&ticket.0))),
        input: Some(ApprovalInput {
            begin,
            checked,
            repository,
            executable: deps.executable.clone(),
            today,
            now,
        }),
        ..Default::default()
    };
    let slot = Arc::clone(&deps.operations);
    #[cfg(all(
        test,
        feature = "full-tests",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    let control = tests::take_dispatch();
    // Keep the input in this same mutex through thread creation. The spawned
    // closure cannot take it until its real JoinHandle is installed here.
    match operation::spawn_thread(move || {
        #[cfg(all(
            test,
            feature = "full-tests",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        tests::install_dispatch(control);
        run_pending(&slot, &ticket);
    }) {
        Ok(worker) => state.archive.approval.worker = Some(worker),
        Err(_) => {
            let input = state
                .archive
                .approval
                .input
                .take()
                .expect("original pending input");
            state.archive.approval.unused.push(UnusedApproval {
                input,
                refusal: UnusedRefusal::Start,
            });
        }
    }
    Ok(ApprovalStart {
        generation: generation.to_string(),
    })
}

fn current_matches(state: &OperationState, ticket: &ArchiveTicket) -> bool {
    state.archive.matches(ticket)
        && state.archive.approval.generation == Some(ticket.generation())
        && state.current.as_ref().is_some_and(|current| {
            current.generation == ticket.generation()
                && current.view.kind == KIND
                && current.handoff_poll.is_none()
                && current.pending_handoff_terminal.is_none()
                && {
                    #[cfg(windows)]
                    {
                        current.cleanup.is_none()
                    }
                    #[cfg(not(windows))]
                    {
                        true
                    }
                }
        })
}

fn retain_unused(slot: &SharedOperationSlot, input: ApprovalInput, refusal: UnusedRefusal) {
    // Poison recovery conserves custody only; status/response refuse it.
    slot.lock()
        .unwrap_or_else(|error| error.into_inner())
        .archive
        .approval
        .unused
        .push(UnusedApproval { input, refusal });
}

fn run_pending(slot: &SharedOperationSlot, ticket: &ArchiveTicket) {
    #[cfg(all(
        test,
        feature = "full-tests",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    tests::before_pending();
    let input = {
        let mut state = match slot.lock() {
            Ok(state) => state,
            Err(error) => {
                let mut state = error.into_inner();
                if state
                    .archive
                    .approval
                    .input
                    .as_ref()
                    .is_some_and(|input| Arc::ptr_eq(&input.begin.ticket.0, &ticket.0))
                {
                    let input = state.archive.approval.input.take().expect("matching input");
                    state.archive.approval.unused.push(UnusedApproval {
                        input,
                        refusal: UnusedRefusal::Start,
                    });
                }
                return;
            }
        };
        let ready = current_matches(&state, ticket)
            && state.current.as_ref().is_some_and(|current| {
                !current.worker_active && !operation::is_terminal(&current.view.phase)
            });
        let Some(input) = state
            .archive
            .approval
            .input
            .take_if(|input| Arc::ptr_eq(&input.begin.ticket.0, &ticket.0))
        else {
            return;
        };
        if !ready {
            state.archive.approval.unused.push(UnusedApproval {
                input,
                refusal: UnusedRefusal::Start,
            });
            return;
        }
        input
    };
    // The whole original input stays outside this borrowed unwind boundary.
    let resolution = catch_unwind(AssertUnwindSafe(|| {
        input
            .begin
            .budget
            .remaining(input.begin.ticket.0.clock.as_ref())
            .map_err(UnusedRefusal::Budget)?;
        if input.repository.is_none() {
            return Ok(None);
        }
        #[cfg(all(
            test,
            feature = "full-tests",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        tests::before_resolver();
        super::resolve_read_invocation(&input.begin.ticket.0.root, &input.executable)
            .map(Some)
            .map_err(UnusedRefusal::Tools)
    }));
    let restic = match resolution {
        Ok(Ok(restic)) => restic,
        Ok(Err(refusal)) => return retain_unused(slot, input, refusal),
        Err(_) => return retain_unused(slot, input, UnusedRefusal::ResolverUnknown),
    };
    let ApprovalInput {
        begin,
        checked,
        repository,
        today,
        now,
        ..
    } = input;
    let (Some(repository), Some(restic)) = (repository, restic) else {
        return remove_local(slot, begin, checked, today, now);
    };
    let generation = begin.generation();
    match prepare_read(slot, begin, repository, restic.invocation()) {
        Some(worker) => remove(slot, worker, checked, today, now),
        None => {
            // A parked whole failure keeps the selected context beside it rather
            // than manufacturing a retry; a released one leaves nothing to keep.
            let mut state = slot.lock().unwrap_or_else(|error| error.into_inner());
            if state
                .archive
                .retained
                .iter()
                .any(|returned| returned.generation() == generation)
            {
                state.archive.approval.refused_selection = Some(checked);
            }
        }
    }
}

#[derive(Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ApprovalState {
    Running,
    Refused,
    Held,
    Unknown,
    Completed,
}

#[derive(Debug, Serialize)]
pub struct ApprovalStatus {
    pub generation: String,
    pub state: ApprovalState,
    pub worker_settled: bool,
    pub cancellation_requested: bool,
    pub actor_phase: Option<String>,
    pub reason_code: Option<String>,
    pub report: Option<Value>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ApprovalStatusRefusal {
    Unavailable,
    Stale,
}

fn generation(value: &str) -> Result<u64, ApprovalStatusRefusal> {
    value
        .parse::<u64>()
        .ok()
        .filter(|id| id.to_string() == value)
        .ok_or(ApprovalStatusRefusal::Stale)
}

fn unused_reason(refusal: &UnusedRefusal) -> String {
    match refusal {
        UnusedRefusal::Start => "worker_start_failed".into(),
        UnusedRefusal::Budget(error) => match error {
            AdmissionError::ConfigurationUnavailable => "configuration_unavailable",
            AdmissionError::ConfigurationChanged => "configuration_changed",
            AdmissionError::JournalIdentityChanged => "journal_identity_changed",
            AdmissionError::DeadlineExceeded => "deadline_exceeded",
            AdmissionError::ClockChanged => "clock_changed",
            AdmissionError::CredentialsExpired => "credentials_expired",
            AdmissionError::Cancelled => "cancelled",
        }
        .into(),
        UnusedRefusal::Tools(error) => error.to_string(),
        UnusedRefusal::ResolverUnknown => "resolver_unknown".into(),
    }
}

fn snapshot(
    state: &OperationState,
    generation: u64,
) -> Result<ApprovalStatus, ApprovalStatusRefusal> {
    let current = state
        .current
        .as_ref()
        .filter(|current| {
            current.generation == generation
                && current.view.kind == KIND
                && state.archive.approval.generation == Some(generation)
        })
        .ok_or(ApprovalStatusRefusal::Stale)?;
    let approval = &state.archive.approval;
    if !approval.accepted_completion
        && approval.discharged_refusal.is_none()
        && !approval
            .ticket
            .as_ref()
            .is_some_and(|ticket| current_matches(state, ticket))
    {
        return Err(ApprovalStatusRefusal::Stale);
    }
    let mut result = ApprovalStatus {
        generation: generation.to_string(),
        state: ApprovalState::Running,
        worker_settled: approval.settled(),
        cancellation_requested: current
            .archive_cancellation
            .as_ref()
            .is_some_and(|flag| flag.is_cancelled()),
        actor_phase: None,
        reason_code: None,
        report: None,
    };
    if let Some(refusal) = &approval.discharged_refusal {
        result.state = ApprovalState::Refused;
        result.reason_code = Some(refusal.clone());
        return Ok(result);
    }
    if let Some(unused) = approval
        .unused
        .iter()
        .find(|unused| unused.input.begin.generation() == generation)
    {
        if !current_matches(state, &unused.input.begin.ticket) {
            return Err(ApprovalStatusRefusal::Stale);
        }
        result.state = ApprovalState::Refused;
        result.reason_code = Some(unused_reason(&unused.refusal));
        return Ok(result);
    }
    if let Some(returned) = state
        .archive
        .retained
        .iter()
        .find(|returned| returned.generation() == generation)
    {
        if !current_matches(state, &returned.ticket) {
            return Err(ApprovalStatusRefusal::Stale);
        }
        result.state = match &returned.payload {
            ArchivePayload::EarlyUnknown
            | ArchivePayload::ProofFinishUnknown { .. }
            | ArchivePayload::ProofUnknown { .. }
            | ArchivePayload::OrdinaryUnknown
            | ArchivePayload::Unknown { .. } => ApprovalState::Unknown,
            ArchivePayload::ProofFinished { runtime, .. }
                if runtime.state()
                    == solstone_core_backup_runtime::DirectReadFinishState::Unknown =>
            {
                ApprovalState::Unknown
            }
            _ => ApprovalState::Held,
        };
        result.actor_phase = Some(returned.terminal.phase.clone());
        result.reason_code = returned.terminal.reason_code.clone();
        result.report = returned.terminal.report.clone();
    } else if approval.accepted_completion {
        // This marker is installed only by handoff after genuine Runtime
        // completion, original custody identity and fresh binding succeed.
        result.state = if result.worker_settled {
            ApprovalState::Completed
        } else {
            ApprovalState::Running
        };
        result.actor_phase = Some(current.view.phase.clone());
        result.reason_code = current.view.reason_code.clone();
        result.report = current.view.report.clone();
    } else if let Some(input) = approval.input.as_ref()
        && !current_matches(state, &input.begin.ticket)
    {
        return Err(ApprovalStatusRefusal::Stale);
    }
    Ok(result)
}

/// Observe only the matching generation; never substitute a successor's facts.
pub fn status(
    slot: &SharedOperationSlot,
    id: &str,
) -> Result<ApprovalStatus, ApprovalStatusRefusal> {
    let generation = generation(id)?;
    let state = slot
        .lock()
        .map_err(|_| ApprovalStatusRefusal::Unavailable)?;
    snapshot(&state, generation)
}

/// Serialize under the same mutex before discharging a genuine unused input.
/// Admitted Held/Unknown and rejected returns are never released here.
pub fn response(slot: &SharedOperationSlot, id: &str) -> Result<Response, ApprovalStatusRefusal> {
    let generation = generation(id)?;
    let mut state = slot
        .lock()
        .map_err(|_| ApprovalStatusRefusal::Unavailable)?;
    locked_response(&mut state, generation)
}

/// Negatively signal only this still-reserved approval's original flag.
/// Validation, signaling and response custody share the same admission mutex.
pub fn cancel_response(
    slot: &SharedOperationSlot,
    id: &str,
) -> Result<Response, ApprovalStatusRefusal> {
    let generation = generation(id)?;
    let mut state = slot
        .lock()
        .map_err(|_| ApprovalStatusRefusal::Unavailable)?;
    let approval = &state.archive.approval;
    let matches = approval.generation == Some(generation)
        && !approval.accepted_completion
        && approval.discharged_refusal.is_none()
        && approval
            .ticket
            .as_ref()
            .is_some_and(|ticket| current_matches(&state, ticket));
    if !matches {
        return Err(ApprovalStatusRefusal::Stale);
    }
    state
        .current
        .as_ref()
        .and_then(|current| current.archive_cancellation.as_ref())
        .ok_or(ApprovalStatusRefusal::Unavailable)?
        .cancel();
    locked_response(&mut state, generation)
}

fn locked_response(
    state: &mut OperationState,
    generation: u64,
) -> Result<Response, ApprovalStatusRefusal> {
    let snapshot = snapshot(state, generation)?;
    let serialized = catch_unwind(AssertUnwindSafe(|| {
        #[cfg(all(
            test,
            feature = "full-tests",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        tests::before_response();
        Json(&snapshot).into_response()
    }))
    .map_err(|_| ApprovalStatusRefusal::Unavailable)?;
    if !serialized.status().is_success() {
        return Err(ApprovalStatusRefusal::Unavailable);
    }
    if snapshot.worker_settled && !state.archive.approval.unused.is_empty() {
        let unused = &state.archive.approval.unused;
        if unused.len() != 1
            || !current_matches(state, &unused[0].input.begin.ticket)
            || state
                .current
                .as_ref()
                .is_some_and(|current| current.worker_active)
        {
            return Err(ApprovalStatusRefusal::Unavailable);
        }
        let current = state.current.as_mut().expect("matching current");
        operation::apply_terminal(
            current,
            Terminal::error(
                snapshot
                    .reason_code
                    .clone()
                    .unwrap_or_else(|| "failed".into()),
            ),
        );
        state.archive.approval.discharged_refusal = snapshot.reason_code;
        drop(state.archive.approval.unused.pop());
        state.archive.reservation = None;
    }
    Ok(serialized)
}

/// Called synchronously inside the worker holding the original admission.
pub fn remove(
    slot: &SharedOperationSlot,
    worker: PreparedArchiveWorker,
    checked: CheckedReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
) {
    // The outside owner never enters the actor's unwind boundary.
    let PreparedArchiveWorker {
        ticket,
        custody,
        read,
        outside,
    } = worker;
    if let Err(reason) = entry(slot, &ticket, &read, &outside) {
        return return_owned(
            slot,
            ticket,
            Some(custody),
            ArchivePayload::ReadHeld {
                read: Box::new(read.into_owned_hold()),
                outside,
            },
            Terminal::error(reason),
        );
    }
    let mut terminal = Terminal::error("failed");
    let actor = catch_unwind(AssertUnwindSafe(|| {
        let proof = remove_reviewed_with_owned_archive(checked, read, today, now);
        project(
            &mut terminal,
            proof.outcome(),
            serde_json::to_value(removal_report::owned_archive(&proof))
                .expect("borrowed removal facts serialize"),
        );
        #[cfg(all(
            test,
            feature = "full-tests",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        tests::after_actor(&proof);
        match proof.finish() {
            OwnedReviewedArchiveFinish::Held(proof) => {
                project(
                    &mut terminal,
                    proof.outcome(),
                    serde_json::to_value(removal_report::held_archive(&proof))
                        .expect("closed removal facts serialize"),
                );
                terminal.phase = if proof.outcome().removed_paths().next().is_some() {
                    "degraded"
                } else {
                    "refused"
                }
                .into();
                terminal.reason_code = Some("failed".into());
                // A removal that changed nothing ends here once its own stages
                // are gone and its children reaped; anything else stays held.
                let proof = match proof.release_refused() {
                    Ok((read, report)) => {
                        if let Some(view) = terminal.report.as_mut() {
                            view["settlement"] = json!({"state":"refused"});
                        }
                        return Ok(Actor::Refused(Box::new(read), report));
                    }
                    Err(proof) => proof,
                };
                if let Some(report) = terminal.report.as_mut() {
                    report["settlement"] = json!({"state":"held"});
                }
                Err(proof)
            }
            OwnedReviewedArchiveFinish::Settled(proof) => {
                let (read, report) = proof.into_parts();
                project(
                    &mut terminal,
                    report.outcome(),
                    serde_json::to_value(removal_report::settled_archive(&report))
                        .expect("settled removal facts serialize"),
                );
                if let Some(view) = terminal.report.as_mut() {
                    view["settlement"] = json!({"state":"settled"});
                }
                Ok(Actor::Settled(Box::new(read), report))
            }
        }
    }));
    let payload = match actor {
        Ok(Err(proof)) => ArchivePayload::ProofHeld { proof, outside },
        Ok(Ok(Actor::Refused(read, report))) => {
            match catch_unwind(AssertUnwindSafe(|| outside.finish_refused(*read))) {
                Ok(runtime) => {
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = runtime_facts(&runtime);
                    }
                    ArchivePayload::ProofFinished { report, runtime }
                }
                Err(_) => {
                    terminal.phase = "error".into();
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = json!({"state":"unknown"});
                    }
                    ArchivePayload::ProofFinishUnknown { report }
                }
            }
        }
        Ok(Ok(Actor::Settled(read, report))) => {
            #[cfg(all(
                test,
                feature = "full-tests",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            tests::before_finish(slot);
            // Keep the genuine settled report and original custody outside
            // consuming Runtime finish; an unwind cannot certify retirement.
            match catch_unwind(AssertUnwindSafe(|| outside.finish_owned(*read))) {
                Ok(runtime) => {
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = runtime_facts(&runtime);
                    }
                    #[cfg(all(
                        test,
                        feature = "full-tests",
                        any(target_os = "linux", target_os = "macos", target_os = "windows")
                    ))]
                    tests::before_handoff(slot);
                    ArchivePayload::ProofFinished { report, runtime }
                }
                Err(_) => {
                    terminal.phase = "error".into();
                    terminal.reason_code = Some("failed".into());
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = json!({"state":"unknown"});
                    }
                    ArchivePayload::ProofFinishUnknown { report }
                }
            }
        }
        Err(_) => {
            let runtime = outside.into_unknown();
            if let Some(report) = terminal.report.as_mut() {
                report["settlement"] = json!({"state":"unknown"});
                report["runtime"] = runtime_facts(&runtime);
            }
            terminal.phase = "error".into();
            terminal.reason_code = Some("failed".into());
            ArchivePayload::ProofUnknown { runtime }
        }
    };
    return_owned(slot, ticket, Some(custody), payload, terminal);
}

enum Actor {
    Settled(Box<OwnedSettledDirectRead>, ReviewedArchiveReport),
    Refused(Box<RefusedDirectRead>, ReviewedArchiveReport),
}

/// Release a policy-only selection locally in the same admitted slot. The
/// executor takes its own activity; any hold stays parked in this slot.
fn remove_local(
    slot: &SharedOperationSlot,
    begin: OwnedArchiveBegin,
    checked: CheckedReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
) {
    let ticket = ArchiveTicket(Arc::clone(&begin.ticket.0));
    let admitted =
        if !start_owned_worker(slot, &ticket) || !std::ptr::eq(checked.root(), &*ticket.0.root) {
            Err("failed")
        } else {
            entry_current(slot, &ticket)
        };
    if let Err(reason) = admitted {
        return return_owned(
            slot,
            ticket,
            None,
            ArchivePayload::BeginHeld(Box::new(begin)),
            Terminal::error(reason),
        );
    }
    drop(begin);
    let mut terminal = Terminal::error("failed");
    let actor = catch_unwind(AssertUnwindSafe(|| {
        let removal = remove_reviewed(checked, today, now);
        project(
            &mut terminal,
            removal.outcome(),
            serde_json::to_value(removal_report::ordinary(&removal))
                .expect("borrowed removal facts serialize"),
        );
        match removal.finish() {
            ReviewedRemovalFinish::Held(held) => {
                project(
                    &mut terminal,
                    held.removal().outcome(),
                    serde_json::to_value(removal_report::held_ordinary(&held))
                        .expect("closed removal facts serialize"),
                );
                terminal.phase = if held.removal().outcome().removed_paths().next().is_some() {
                    "degraded"
                } else {
                    "refused"
                }
                .into();
                terminal.reason_code = Some("failed".into());
                if let Some(report) = terminal.report.as_mut() {
                    report["settlement"] = json!({"state":"held"});
                }
                ArchivePayload::OrdinaryHeld(held)
            }
            ReviewedRemovalFinish::Settled(report) => {
                project(
                    &mut terminal,
                    report.outcome(),
                    serde_json::to_value(removal_report::settled_ordinary(&report))
                        .expect("settled removal facts serialize"),
                );
                if let Some(view) = terminal.report.as_mut() {
                    view["settlement"] = json!({"state":"settled"});
                }
                ArchivePayload::OrdinarySettled(report)
            }
        }
    }));
    let payload = actor.unwrap_or_else(|_| {
        terminal.phase = "error".into();
        terminal.reason_code = Some("failed".into());
        if let Some(report) = terminal.report.as_mut() {
            report["settlement"] = json!({"state":"unknown"});
        }
        ArchivePayload::OrdinaryUnknown
    });
    return_owned(slot, ticket, None, payload, terminal);
}

fn project(terminal: &mut Terminal, outcome: &Outcome, report: Value) {
    let phase = if outcome.halted.is_some() {
        "error"
    } else if outcome.has_failures() {
        "degraded"
    } else {
        "done"
    };
    *terminal = Terminal::phase(phase, (phase != "done").then(|| "failed".into()));
    terminal.report = Some(report);
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod tests;
