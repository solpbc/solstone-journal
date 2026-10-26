// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Run an already admitted restore and retain its whole owning return.

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
use crate::operation;
#[cfg(all(
    test,
    feature = "full-tests",
    any(target_os = "linux", target_os = "macos")
))]
use solstone_core_backup_runtime::DirectReadFinishState;
use std::panic::{AssertUnwindSafe, catch_unwind};
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
use std::sync::Arc;

use serde_json::{Value, json};
use solstone_core_backup::archive_readback::{OwnedSettledDirectRead, RefusedDirectRead};
use solstone_core_offload::restore::RestoreResult;
use solstone_core_offload::restore::owned::{
    self, OwnedRestoreFinishHold, OwnedRestoreRunOutcome, RestoreFinishRefusal,
    SettledRestoreReport,
};

use super::{ArchivePayload, PreparedArchiveWorker, entry, return_owned, runtime_facts};
use crate::operation::{SharedOperationSlot, Terminal};
use crate::restore_report;

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
use {
    super::{ArchiveTicket, OwnedArchiveBegin, begin_owned_locked, prepare_read_in},
    crate::{BackupWebDeps, operation::OperationState},
    solstone_core_backup::archive_admission::AdmissionError,
    solstone_core_backup_runtime::ClosedToolError,
    solstone_core_journal_io::JournalRoot,
    solstone_core_offload::restore::selected_repository as repository,
    std::{path::PathBuf, thread::JoinHandle},
};

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
struct Input {
    begin: OwnedArchiveBegin,
    repository: Option<String>,
    day: Option<String>,
    time: u64,
    executable: PathBuf,
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
struct Unused {
    input: Input,
    refusal: UnusedRefusal,
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
enum UnusedRefusal {
    Start,
    Budget(AdmissionError),
    Tools(ClosedToolError),
    ResolverUnknown,
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
impl UnusedRefusal {
    fn reason(&self) -> String {
        match self {
            Self::Start => "worker_start_failed".into(),
            Self::ResolverUnknown => "resolver_unknown".into(),
            Self::Tools(error) => error.to_string(),
            Self::Budget(error) => match error {
                AdmissionError::ConfigurationUnavailable => "configuration_unavailable",
                AdmissionError::ConfigurationChanged => "configuration_changed",
                AdmissionError::JournalIdentityChanged => "journal_identity_changed",
                AdmissionError::DeadlineExceeded => "deadline_exceeded",
                AdmissionError::ClockChanged => "clock_changed",
                AdmissionError::CredentialsExpired => "credentials_expired",
                AdmissionError::Cancelled => "cancelled",
            }
            .into(),
        }
    }
}

#[derive(Default)]
pub(super) struct Custody {
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    ticket: Option<ArchiveTicket>,
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    input: Option<Input>,
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    unused: Vec<Unused>,
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    worker: Option<JoinHandle<()>>,
}

impl Custody {
    pub(super) fn needs_settlement(&self) -> bool {
        #[cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        ))]
        {
            !self.unused.is_empty()
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        )))]
        false
    }
    pub(super) fn worker_running(&self) -> bool {
        #[cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        ))]
        {
            self.worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        )))]
        false
    }
    pub(super) fn is_busy(&self) -> bool {
        #[cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        ))]
        {
            self.input.is_some()
                || !self.unused.is_empty()
                || self
                    .worker
                    .as_ref()
                    .is_some_and(|worker| !worker.is_finished())
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        )))]
        false
    }

    pub(super) fn take_finished_worker(&mut self) -> Option<std::thread::JoinHandle<()>> {
        #[cfg(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        ))]
        {
            if self.worker.as_ref().is_some_and(JoinHandle::is_finished) {
                self.worker.take()
            } else {
                None
            }
        }
        #[cfg(not(any(
            target_os = "linux",
            target_os = "macos",
            target_os = "windows",
            target_os = "ios",
            target_os = "android"
        )))]
        None
    }
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn refuse(reason: &str) -> axum::response::Response {
    crate::response::error(
        axum::http::StatusCode::BAD_REQUEST,
        "that action isn't available in the current state.",
        reason,
        "",
    )
}

/// Reserve the original input before an installed restore worker can start.
#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
pub(crate) fn start(
    deps: &BackupWebDeps,
    day: Option<String>,
) -> Result<u64, axum::response::Response> {
    let root: Arc<JournalRoot> =
        super::retained_archive_root(deps).map_err(|_| refuse("restore_held"))?;
    let repository = repository(&root, day.as_deref()).map_err(refuse)?;
    let time = u64::try_from(deps.clock.now_unix()).map_err(|_| refuse("failed"))?;
    let mut state =
        operation::lock_for_begin(&deps.operations).map_err(|()| operation::busy_response())?;
    if state.closed {
        return Err(operation::busy_response());
    }
    root.revalidate_canonical_binding()
        .map_err(|_| refuse("restore_held"))?;
    let begin = begin_owned_locked(&mut state, "offload_restore", root, Arc::clone(&deps.clock))
        .map_err(|_| refuse("restore_held"))?;
    let generation = begin.generation();
    let ticket = ArchiveTicket(Arc::clone(&begin.ticket.0));
    state.archive.restore = Custody {
        ticket: Some(ArchiveTicket(Arc::clone(&ticket.0))),
        input: Some(Input {
            begin,
            repository,
            day,
            time,
            executable: deps.executable.clone(),
        }),
        ..Default::default()
    };
    let slot = Arc::clone(&deps.operations);
    #[cfg(all(test, feature = "full-tests"))]
    let control = tests::take_dispatch();
    match operation::spawn_thread(move || {
        #[cfg(all(test, feature = "full-tests"))]
        tests::install_dispatch(control);
        run_pending(&slot, &ticket);
    }) {
        Ok(worker) => state.archive.restore.worker = Some(worker),
        Err(_) => {
            let input = state
                .archive
                .restore
                .input
                .take()
                .expect("original restore input");
            state.archive.restore.unused.push(Unused {
                input,
                refusal: UnusedRefusal::Start,
            });
        }
    }
    Ok(generation)
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn current_matches(state: &OperationState, ticket: &ArchiveTicket) -> bool {
    state.archive.matches(ticket)
        && state
            .archive
            .restore
            .ticket
            .as_ref()
            .is_some_and(|original| Arc::ptr_eq(&original.0, &ticket.0))
        && state.current.as_ref().is_some_and(|current| {
            current.generation == ticket.generation()
                && current.view.kind == "offload_restore"
                && current.handoff_poll.is_none()
                && current.pending_handoff_terminal.is_none()
        })
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn retain_unused(slot: &SharedOperationSlot, input: Input, refusal: UnusedRefusal) {
    slot.lock()
        .unwrap_or_else(|error| error.into_inner())
        .archive
        .restore
        .unused
        .push(Unused { input, refusal });
}

#[cfg(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows",
    target_os = "ios",
    target_os = "android"
))]
fn run_pending(slot: &SharedOperationSlot, ticket: &ArchiveTicket) {
    #[cfg(all(test, feature = "full-tests"))]
    tests::before_pending();
    let input = {
        let (mut state, poisoned) = match slot.lock() {
            Ok(state) => (state, false),
            Err(error) => (error.into_inner(), true),
        };
        let ready = !poisoned
            && current_matches(&state, ticket)
            && state.current.as_ref().is_some_and(|current| {
                !current.worker_active && !operation::is_terminal(&current.view.phase)
            });
        let Some(input) = state
            .archive
            .restore
            .input
            .take_if(|input| Arc::ptr_eq(&input.begin.ticket.0, &ticket.0))
        else {
            return;
        };
        if !ready {
            state.archive.restore.unused.push(Unused {
                input,
                refusal: UnusedRefusal::Start,
            });
            return;
        }
        input
    };
    // Original input remains outside the borrowed resolver unwind boundary.
    let resolution = catch_unwind(AssertUnwindSafe(|| {
        input
            .begin
            .budget
            .remaining(input.begin.ticket.0.clock.as_ref())
            .map_err(UnusedRefusal::Budget)?;
        #[cfg(all(test, feature = "full-tests"))]
        tests::before_resolver();
        super::resolve_read_invocation(&input.begin.ticket.0.root, &input.executable)
            .map_err(UnusedRefusal::Tools)
    }));
    let restic = match resolution {
        Ok(Ok(restic)) => restic,
        Ok(Err(refusal)) => return retain_unused(slot, input, refusal),
        Err(_) => return retain_unused(slot, input, UnusedRefusal::ResolverUnknown),
    };
    let Input {
        begin,
        repository,
        day: selection,
        time,
        ..
    } = input;
    if let Some(worker) = prepare_read_in(slot, begin, repository, restic.invocation()) {
        match selection {
            Some(selection) => day(slot, worker, &selection, time),
            None => all(slot, worker, time),
        }
    }
}

/// Serialize a matching unused input before releasing it under admission lock.
pub(crate) fn response(
    deps: &crate::BackupWebDeps,
    expected: Option<u64>,
) -> Option<axum::response::Response> {
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    {
        let _ = (deps, expected);
        None
    }
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    {
        let mut state = match deps.operations.lock() {
            Ok(state) => state,
            Err(_) => return Some(crate::internal_error()),
        };
        let pending = &state.archive.restore;
        if expected.is_none() && pending.unused.is_empty() {
            return None;
        }
        let ticket = match &pending.ticket {
            Some(ticket) => ticket,
            None => return Some(crate::internal_error()),
        };
        let Some(current) = state.current.as_ref().filter(|current| {
            current.generation == ticket.generation()
                && expected.is_none_or(|id| id == current.generation)
                && current.view.kind == "offload_restore"
        }) else {
            return Some(crate::internal_error());
        };
        if !Arc::ptr_eq(&ticket.0.clock, &deps.clock)
            || ticket.0.root.revalidate_canonical_binding().is_err()
        {
            return Some(crate::internal_error());
        }
        let mut view = current.view.clone();
        let discharge = !pending.unused.is_empty()
            && pending
                .worker
                .as_ref()
                .is_none_or(|worker| worker.is_finished());
        if discharge {
            if pending.unused.len() != 1
                || !current_matches(&state, ticket)
                || current.worker_active
                || operation::is_terminal(&current.view.phase)
                || pending
                    .worker
                    .as_ref()
                    .is_some_and(|worker| !worker.is_finished())
                || !state.archive.retained.is_empty()
                || !Arc::ptr_eq(&pending.unused[0].input.begin.ticket.0, &ticket.0)
            {
                return Some(crate::internal_error());
            }
            let unused = &pending.unused[0];
            view.phase = "refused".into();
            let reason = unused.refusal.reason();
            if let UnusedRefusal::Tools(error) = &unused.refusal {
                view.detail = Some(error.detail().into());
                view.guidance = Some(error.guidance().into());
            }
            view.reason_code = Some(reason.clone());
            view.report = Some(json!({"status":"refused", "reason":reason,
                "scope":if unused.input.day.is_some() {"day"} else {"all"},
                "day":unused.input.day,"files_restored":0,"bytes_restored":0}));
        }
        let response = catch_unwind(AssertUnwindSafe(|| {
            #[cfg(all(test, feature = "full-tests"))]
            tests::before_response();
            crate::status::status_with_operation(&deps.journal_root, Some(&view))
                .map(crate::response::success)
        }));
        match response {
            Ok(Ok(response)) if response.status().is_success() => {
                if discharge {
                    state.current.as_mut().expect("matching restore").view = view;
                    drop(state.archive.restore.unused.pop());
                    state.archive.reservation = None;
                }
                Some(response)
            }
            _ => Some(crate::internal_error()),
        }
    }
}

/// Called synchronously inside the worker holding the original admission.
pub(super) fn settle_closed(state: &mut crate::operation::OperationState) {
    #[cfg(not(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    )))]
    let _ = state;
    #[cfg(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows",
        target_os = "ios",
        target_os = "android"
    ))]
    {
        let pending = &state.archive.restore;
        if !state.closed
            || pending.unused.len() != 1
            || pending
                .worker
                .as_ref()
                .is_some_and(|worker| !worker.is_finished())
            || !state.archive.retained.is_empty()
        {
            return;
        }
        let unused = &pending.unused[0];
        let ticket = &unused.input.begin.ticket;
        if !current_matches(state, ticket)
            || state
                .current
                .as_ref()
                .is_some_and(|current| current.worker_active)
            || ticket.0.root.revalidate_canonical_binding().is_err()
        {
            return;
        }
        let mut view = state
            .current
            .as_ref()
            .expect("matching restore")
            .view
            .clone();
        if !operation::is_terminal(&view.phase) {
            let reason = unused.refusal.reason();
            view.phase = "refused".into();
            view.reason_code = Some(reason.clone());
            if let UnusedRefusal::Tools(error) = &unused.refusal {
                view.detail = Some(error.detail().into());
                view.guidance = Some(error.guidance().into());
            }
            view.report = Some(json!({"status":"refused", "reason":reason,
                "scope":if unused.input.day.is_some() {"day"} else {"all"},
                "day":unused.input.day,"files_restored":0,"bytes_restored":0}));
        }
        state.current.as_mut().expect("matching restore").view = view;
        drop(state.archive.restore.unused.pop());
        state.archive.reservation = None;
    }
}

/// Called synchronously inside the worker holding the original admission.
pub fn day(slot: &SharedOperationSlot, worker: PreparedArchiveWorker, day: &str, now: u64) {
    run(slot, worker, Some(day), now);
}

/// Called synchronously inside the worker holding the original admission.
pub fn all(slot: &SharedOperationSlot, worker: PreparedArchiveWorker, now: u64) {
    run(slot, worker, None, now);
}

fn run(slot: &SharedOperationSlot, worker: PreparedArchiveWorker, day: Option<&str>, now: u64) {
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
        let native = match day {
            Some(day) => owned::restore_owned_day(read, day, now),
            None => owned::restore_owned_all(read, now),
        };
        // This snapshot remains available outside the closure if consuming
        // cleanup unwinds before a final held/settled projection returns.
        project(
            &mut terminal,
            native.report(),
            restore_report::active(&native),
        );
        #[cfg(all(
            test,
            feature = "full-tests",
            any(target_os = "linux", target_os = "macos", target_os = "windows")
        ))]
        tests::after_actor(&native);
        match settle(native) {
            Err(native) => {
                project(
                    &mut terminal,
                    native.report(),
                    restore_report::held(&native),
                );
                let refusal = native_refusal(native.refusal());
                // A refused restore ends here once no stage of its own remains
                // and its children were reaped; anything else stays held.
                let native = match native.release_refused() {
                    Ok((read, report)) => {
                        // An unqualified read never presents as done.
                        if terminal.phase == "done" {
                            terminal.phase = "error".into();
                            terminal.reason_code = Some("failed".into());
                        }
                        if let Some(view) = terminal.report.as_mut() {
                            view["settlement"] = json!({"state":"refused", "refusal":refusal});
                        }
                        return Ok(Actor::Refused(Box::new(read), report));
                    }
                    Err(native) => native,
                };
                if let Some(report) = terminal.report.as_mut() {
                    report["settlement"] = json!({"state":"held", "refusal":refusal});
                }
                Err(native)
            }
            Ok((settled, report)) => {
                project(
                    &mut terminal,
                    report.report(),
                    restore_report::settled(&report),
                );
                if let Some(view) = terminal.report.as_mut() {
                    view["settlement"] = json!({"state":"settled"});
                }
                Ok(Actor::Settled(Box::new(settled), report))
            }
        }
    }));
    let payload = match actor {
        Ok(Err(native)) => ArchivePayload::NativeHeld { native, outside },
        Ok(Ok(Actor::Refused(read, report))) => {
            match catch_unwind(AssertUnwindSafe(|| outside.finish_refused(*read))) {
                Ok(runtime) => {
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = runtime_facts(&runtime);
                    }
                    ArchivePayload::NativeFinished { report, runtime }
                }
                Err(_) => {
                    terminal.phase = "error".into();
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = json!({"state":"unknown"});
                    }
                    ArchivePayload::NativeFinishUnknown { report }
                }
            }
        }
        Ok(Ok(Actor::Settled(settled, report))) => {
            #[cfg(all(
                test,
                feature = "full-tests",
                any(target_os = "linux", target_os = "macos", target_os = "windows")
            ))]
            tests::before_finish(slot);
            // The genuine settled report and original custody survive a
            // consuming Runtime-finish unwind independently of confirmation.
            match catch_unwind(AssertUnwindSafe(|| outside.finish_owned(*settled))) {
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
                    ArchivePayload::NativeFinished { report, runtime }
                }
                Err(_) => {
                    terminal.phase = "error".into();
                    terminal.reason_code = Some("failed".into());
                    if let Some(view) = terminal.report.as_mut() {
                        view["runtime"] = json!({"state":"unknown"});
                    }
                    ArchivePayload::NativeFinishUnknown { report }
                }
            }
        }
        Err(_) => {
            let runtime = outside.into_unknown();
            // Preserve genuinely projected effects; unavailable native owners
            // and any unreturned settled report remain unknown.
            if let Some(report) = terminal.report.as_mut() {
                report["settlement"] = json!({"state":"unknown"});
                report["runtime"] = runtime_facts(&runtime);
            }
            terminal.phase = "error".into();
            terminal.reason_code = Some("failed".into());
            ArchivePayload::Unknown {
                runtime,
                report: None,
            }
        }
    };
    return_owned(slot, ticket, Some(custody), payload, terminal);
}

enum Actor {
    Settled(Box<OwnedSettledDirectRead>, SettledRestoreReport),
    Refused(Box<RefusedDirectRead>, SettledRestoreReport),
}

fn settle(
    native: OwnedRestoreRunOutcome,
) -> Result<(OwnedSettledDirectRead, SettledRestoreReport), Box<OwnedRestoreFinishHold>> {
    #[cfg(all(
        test,
        feature = "full-tests",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    ))]
    return tests::settle(native);
    #[cfg(not(all(
        test,
        feature = "full-tests",
        any(target_os = "linux", target_os = "macos", target_os = "windows")
    )))]
    native.settle_direct()
}

fn project(terminal: &mut Terminal, result: &RestoreResult, report: Value) {
    let phase = match result.status.as_str() {
        "ok" | "no_op" => "done",
        "refused" => "refused",
        "degraded" => "degraded",
        _ => "error",
    };
    *terminal = Terminal::phase(phase, result.reason.clone());
    terminal.report = Some(report);
}

fn native_refusal(refusal: RestoreFinishRefusal<'_>) -> Value {
    match refusal {
        RestoreFinishRefusal::Native {
            file,
            reason,
            source,
            readback,
        } => json!({
            "kind":"native", "file":file, "reason":reason,
            "error":source.map(|error| error.kind().to_string()),
            "readback":readback.map(|error| error.to_string()),
        }),
        RestoreFinishRefusal::Lower(reason) => json!({"kind":"lower", "reason":reason.to_string()}),
    }
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(any(target_os = "linux", target_os = "macos", target_os = "windows"))]
mod tests;
