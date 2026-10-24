// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Finite creation and planning workers retain their original admission owners.

use std::{
    panic::{AssertUnwindSafe, catch_unwind},
    path::Path,
    sync::Arc,
};

use chrono::{DateTime, FixedOffset, NaiveDate, Utc};
use serde_json::json;
use solstone_core_backup::archive_activity::{
    ArchivePreparationCustody, OwnedArchivePreparationInput, OwnedPreparedArchiveHold,
};
use solstone_core_backup_runtime::{
    DirectReadFinishOwner, DirectReadFinishState, OwnedAdmittedArchiveHold,
    OwnedArchiveAdmissionFailure, OwnedDirectReadFinish, UreqHttpTransport,
    admit_prepared_archive_owned_creator_installed, resolve_rclone_member,
};
use solstone_core_journal_io::media::RootActivityCarrier;
use solstone_core_offload::run::{
    self, OwnedOffloadCreationFinish, OwnedOffloadCreationHold, OwnedOffloadInputFailure,
    OwnedOffloadInputs, OwnedOffloadPlanFinish, OwnedOffloadPlanHold, OwnedOffloadPlanInputFailure,
    OwnedOffloadPlanInputs, SettledOffloadCreationReport, SettledOffloadPlanReport,
};
use solstone_core_retention::archive_offer::OffloadOfferSelection;

use super::{
    ArchivePayload, ArchiveTicket, OwnedArchiveBegin, Reservation, entry_current, return_owned,
    runtime_facts, start_owned_worker,
};
use crate::{
    offload_report,
    operation::{SharedOperationSlot, Terminal},
};

enum Request {
    Selected {
        selections: Vec<OffloadOfferSelection>,
        today: NaiveDate,
        now: DateTime<Utc>,
    },
    Planned(DateTime<FixedOffset>),
}

enum Inputs {
    Selected {
        ready: Box<OwnedOffloadInputs>,
        today: NaiveDate,
        now: DateTime<Utc>,
    },
    Planned(Box<OwnedOffloadPlanInputs>),
}
impl Inputs {
    fn carrier(&self) -> &Arc<RootActivityCarrier> {
        match self {
            Self::Selected { ready, .. } => ready.carrier(),
            Self::Planned(ready) => ready.carrier(),
        }
    }
    fn facts(&self) -> serde_json::Value {
        match self {
            Self::Selected { ready, .. } => offload_report::selected_inputs(ready),
            Self::Planned(ready) => offload_report::planned_inputs(ready),
        }
    }
}
enum InputFailure {
    Selected(OwnedOffloadInputFailure),
    Planned(OwnedOffloadPlanInputFailure),
}
impl InputFailure {
    fn carrier(&self) -> &Arc<RootActivityCarrier> {
        match self {
            Self::Selected(failure) => failure.carrier(),
            Self::Planned(failure) => failure.carrier(),
        }
    }
    fn facts(&self) -> serde_json::Value {
        match self {
            Self::Selected(failure) => offload_report::selected_input_failure(failure),
            Self::Planned(failure) => offload_report::planned_input_failure(failure),
        }
    }
}
enum ActorHold {
    Selected(Box<OwnedOffloadCreationHold>),
    Planned(Box<OwnedOffloadPlanHold>),
}
impl ActorHold {
    fn carrier(&self) -> &Arc<RootActivityCarrier> {
        match self {
            Self::Selected(hold) => hold.carrier(),
            Self::Planned(hold) => hold.carrier(),
        }
    }
}
enum SettledReport {
    Selected(Box<SettledOffloadCreationReport>),
    Planned(Box<SettledOffloadPlanReport>),
}

enum ReturnState {
    UnusedRetirement(Box<solstone_core_backup::archive_activity::OwnedUnusedArchiveRetirement>),
    AdmissionClosed(OwnedArchiveAdmissionFailure),
    Refused(OwnedDirectReadFinish),
    Preflight {
        prepared: Box<OwnedPreparedArchiveHold>,
        failure: InputFailure,
    },
    Admission {
        input: Inputs,
        failure: OwnedArchiveAdmissionFailure,
    },
    Entry {
        input: Inputs,
        admitted: Box<OwnedAdmittedArchiveHold>,
        outside: DirectReadFinishOwner,
    },
    Held {
        actor: ActorHold,
        outside: DirectReadFinishOwner,
    },
    Finished {
        report: SettledReport,
        runtime: OwnedDirectReadFinish,
    },
    ActorUnknown {
        runtime: OwnedDirectReadFinish,
    },
    FinishUnknown {
        report: SettledReport,
    },
}

/// Concrete closed custody. Borrowed facts cannot extract or discharge owners.
#[must_use]
pub struct OffloadReturn {
    state: ReturnState,
}

impl Inputs {
    fn unstaged(&self) -> bool {
        match self {
            Self::Selected { ready, .. } => ready.unstaged(),
            Self::Planned(_) => true,
        }
    }
}

impl OffloadReturn {
    /// End a creation refused before any archive writer, copy or record: the
    /// preflight, admission and entry refusals. Every later state holds.
    pub(super) fn release(self) -> Result<(), Box<Self>> {
        let held = |state| Err(Box::new(Self { state }));
        match self.state {
            ReturnState::Preflight { prepared, failure } => {
                let unstaged = match &failure {
                    InputFailure::Selected(failure) => failure.unstaged(),
                    InputFailure::Planned(_) => true,
                };
                if !unstaged || !Arc::ptr_eq(prepared.carrier(), failure.carrier()) {
                    return held(ReturnState::Preflight { prepared, failure });
                }
                drop(failure);
                prepared.retire_unused().map_err(|retirement| {
                    Box::new(Self {
                        state: ReturnState::UnusedRetirement(Box::new(retirement)),
                    })
                })
            }
            ReturnState::Admission { input, failure } if input.unstaged() => {
                if !failure
                    .carrier()
                    .is_some_and(|carrier| Arc::ptr_eq(input.carrier(), carrier))
                {
                    return held(ReturnState::Admission { input, failure });
                }
                // Unstaged planning inputs hold only a shared carrier. Drop
                // them before the actual admission owner attempts retirement.
                drop(input);
                match failure.release() {
                    Ok(()) => Ok(()),
                    Err(failure) => held(ReturnState::AdmissionClosed(*failure)),
                }
            }
            ReturnState::Entry {
                input,
                admitted,
                outside,
            } if input.unstaged() => {
                if !Arc::ptr_eq(input.carrier(), admitted.carrier())
                    || !Arc::ptr_eq(admitted.carrier(), outside.carrier())
                    || !Arc::ptr_eq(admitted.owned_clock(), outside.owned_clock())
                {
                    return held(ReturnState::Entry {
                        input,
                        admitted,
                        outside,
                    });
                }
                match (*admitted).release_refused() {
                    Ok(read) => {
                        drop(input);
                        let runtime = outside.finish_refused(read);
                        if runtime.released() {
                            Ok(())
                        } else {
                            held(ReturnState::Refused(runtime))
                        }
                    }
                    Err(admitted) => held(ReturnState::Entry {
                        input,
                        admitted,
                        outside,
                    }),
                }
            }
            ReturnState::AdmissionClosed(failure) => failure.release().map_err(|failure| {
                Box::new(Self {
                    state: ReturnState::AdmissionClosed(*failure),
                })
            }),
            state => held(state),
        }
    }

    pub fn runtime(&self) -> Option<&OwnedDirectReadFinish> {
        match &self.state {
            ReturnState::Finished { runtime, .. }
            | ReturnState::ActorUnknown { runtime }
            | ReturnState::Refused(runtime) => Some(runtime),
            _ => None,
        }
    }
    pub fn outside(&self) -> Option<&DirectReadFinishOwner> {
        match &self.state {
            ReturnState::Entry { outside, .. } | ReturnState::Held { outside, .. } => Some(outside),
            _ => None,
        }
    }
    pub fn admission_failure(&self) -> Option<&OwnedArchiveAdmissionFailure> {
        match &self.state {
            ReturnState::Admission { failure, .. } | ReturnState::AdmissionClosed(failure) => {
                Some(failure)
            }
            _ => None,
        }
    }
    pub fn selected_input_failure(&self) -> Option<&OwnedOffloadInputFailure> {
        match &self.state {
            ReturnState::Preflight {
                failure: InputFailure::Selected(failure),
                ..
            } => Some(failure),
            _ => None,
        }
    }
    pub fn planned_input_failure(&self) -> Option<&OwnedOffloadPlanInputFailure> {
        match &self.state {
            ReturnState::Preflight {
                failure: InputFailure::Planned(failure),
                ..
            } => Some(failure),
            _ => None,
        }
    }
    pub fn selected_ready(&self) -> Option<&OwnedOffloadInputs> {
        match &self.state {
            ReturnState::Admission {
                input: Inputs::Selected { ready, .. },
                ..
            }
            | ReturnState::Entry {
                input: Inputs::Selected { ready, .. },
                ..
            } => Some(ready),
            _ => None,
        }
    }
    pub fn planned_ready(&self) -> Option<&OwnedOffloadPlanInputs> {
        match &self.state {
            ReturnState::Admission {
                input: Inputs::Planned(ready),
                ..
            }
            | ReturnState::Entry {
                input: Inputs::Planned(ready),
                ..
            } => Some(ready),
            _ => None,
        }
    }
    pub fn creation_hold(&self) -> Option<&OwnedOffloadCreationHold> {
        match &self.state {
            ReturnState::Held {
                actor: ActorHold::Selected(hold),
                ..
            } => Some(hold),
            _ => None,
        }
    }
    pub fn plan_hold(&self) -> Option<&OwnedOffloadPlanHold> {
        match &self.state {
            ReturnState::Held {
                actor: ActorHold::Planned(hold),
                ..
            } => Some(hold),
            _ => None,
        }
    }
    pub fn creation_report(&self) -> Option<&SettledOffloadCreationReport> {
        match &self.state {
            ReturnState::Finished {
                report: SettledReport::Selected(report),
                ..
            }
            | ReturnState::FinishUnknown {
                report: SettledReport::Selected(report),
            } => Some(report),
            _ => None,
        }
    }
    pub fn plan_report(&self) -> Option<&SettledOffloadPlanReport> {
        match &self.state {
            ReturnState::Finished {
                report: SettledReport::Planned(report),
                ..
            }
            | ReturnState::FinishUnknown {
                report: SettledReport::Planned(report),
            } => Some(report),
            _ => None,
        }
    }
    pub(super) fn matches(
        &self,
        reservation: &Reservation,
        custody: Option<&ArchivePreparationCustody>,
    ) -> bool {
        let (root, clock) = match &self.state {
            ReturnState::UnusedRetirement(retirement) => {
                (retirement.journal_root(), Some(retirement.owned_clock()))
            }
            ReturnState::AdmissionClosed(failure) => {
                (failure.journal_root(), failure.owned_clock())
            }
            ReturnState::Refused(runtime) => (runtime.journal_root(), Some(runtime.owned_clock())),
            ReturnState::Preflight { prepared, failure } => {
                if !Arc::ptr_eq(prepared.carrier(), failure.carrier()) {
                    return false;
                }
                (prepared.journal_root(), Some(prepared.owned_clock()))
            }
            ReturnState::Admission { input, failure } => {
                if !failure
                    .carrier()
                    .is_some_and(|carrier| Arc::ptr_eq(input.carrier(), carrier))
                {
                    return false;
                }
                (failure.journal_root(), failure.owned_clock())
            }
            ReturnState::Entry {
                input,
                admitted,
                outside,
            } => {
                if !Arc::ptr_eq(input.carrier(), admitted.carrier())
                    || !Arc::ptr_eq(admitted.carrier(), outside.carrier())
                    || !Arc::ptr_eq(admitted.journal_root(), outside.journal_root())
                    || !Arc::ptr_eq(admitted.owned_clock(), outside.owned_clock())
                {
                    return false;
                }
                (outside.journal_root(), Some(outside.owned_clock()))
            }
            ReturnState::Held { actor, outside } => {
                if !Arc::ptr_eq(actor.carrier(), outside.carrier()) {
                    return false;
                }
                (outside.journal_root(), Some(outside.owned_clock()))
            }
            ReturnState::Finished { runtime, .. } | ReturnState::ActorUnknown { runtime } => {
                (runtime.journal_root(), Some(runtime.owned_clock()))
            }
            ReturnState::FinishUnknown { .. } => {
                let Some(custody) = custody else {
                    return false;
                };
                (custody.journal_root(), Some(custody.owned_clock()))
            }
        };
        Arc::ptr_eq(root, &reservation.root)
            && clock.is_some_and(|clock| Arc::ptr_eq(clock, &reservation.clock))
    }
}

/// Execute selected creation synchronously on the original reserved worker.
pub fn create_selected(
    slot: &SharedOperationSlot,
    begin: OwnedArchiveBegin,
    selections: Vec<OffloadOfferSelection>,
    today: NaiveDate,
    now: DateTime<Utc>,
    restic_path: &Path,
) {
    execute(
        slot,
        begin,
        Request::Selected {
            selections,
            today,
            now,
        },
        restic_path,
    );
}

/// Execute planning synchronously; metadata-only settlement uses the same owner.
pub fn create_planned(
    slot: &SharedOperationSlot,
    begin: OwnedArchiveBegin,
    at: DateTime<FixedOffset>,
    restic_path: &Path,
) {
    execute(slot, begin, Request::Planned(at), restic_path);
}

fn execute(
    slot: &SharedOperationSlot,
    begin: OwnedArchiveBegin,
    request: Request,
    restic_path: &Path,
) {
    if !start_owned_worker(slot, &begin.ticket) {
        let ticket = ArchiveTicket(Arc::clone(&begin.ticket.0));
        return return_owned(
            slot,
            ticket,
            None,
            ArchivePayload::BeginHeld(Box::new(begin)),
            Terminal::error("failed"),
        );
    }
    let (ticket, root, budget, clock) = begin.into_parts();
    let (input, custody) = match OwnedArchivePreparationInput::capture(root, budget, clock) {
        Ok(pair) => pair,
        Err(failure) => {
            let mut terminal = Terminal::error("failed");
            terminal.report = Some(
                json!({"preparation":{"state":"refused","reason":failure.reason().to_string()}}),
            );
            return return_owned(
                slot,
                ticket,
                None,
                ArchivePayload::PreparationFailure(Box::new(failure)),
                terminal,
            );
        }
    };
    let mut terminal = Terminal::error("failed");
    let mut phase = "preparation";
    // Original lease custody stays outside preparation, preflight and admission.
    let admitted = catch_unwind(AssertUnwindSafe(|| {
        let prepared = input.prepare().map_err(|failure| {
            terminal.report = Some(
                json!({"preparation":{"state":"refused","reason":failure.reason().to_string()}}),
            );
            ArchivePayload::PreparationFailure(Box::new(failure))
        })?;
        #[cfg(all(test, feature = "full-tests", target_os = "linux"))]
        tests::prepared();
        phase = "preflight";
        let ready = match match request {
            Request::Selected {
                selections,
                today,
                now,
            } => run::prepare_owned_offload_inputs(&prepared, &selections, today, now)
                .map(|ready| Inputs::Selected {
                    ready: Box::new(ready),
                    today,
                    now,
                })
                .map_err(InputFailure::Selected),
            Request::Planned(at) => run::prepare_owned_offload_plan(&prepared, at)
                .map(|ready| Inputs::Planned(Box::new(ready)))
                .map_err(InputFailure::Planned),
        } {
            Ok(ready) => ready,
            Err(failure) => {
                terminal.report = Some(failure.facts());
                return Err(ArchivePayload::Offload(Box::new(OffloadReturn {
                    state: ReturnState::Preflight {
                        prepared: Box::new(prepared.into_owned_hold()),
                        failure,
                    },
                })));
            }
        };
        terminal.report = Some(ready.facts());
        #[cfg(all(test, feature = "full-tests", target_os = "linux"))]
        tests::after_preflight(slot);
        phase = "admission";
        // An operated writer needs the installed rclone; without it admission
        // refuses before any broker session is issued.
        let rclone = prepared
            .configuration()
            .binding()
            .and_then(|_| std::env::current_exe().ok())
            .and_then(|executable| resolve_rclone_member(&executable).ok());
        match admit_prepared_archive_owned_creator_installed(
            prepared,
            restic_path,
            rclone.as_deref(),
            env!("CARGO_PKG_VERSION"),
            &UreqHttpTransport,
        ) {
            Ok(installed) => Ok((ready, installed)),
            Err(failure) => {
                if let Some(report) = terminal.report.as_mut() {
                    report["admission"] = offload_report::admission_failure(&failure);
                }
                Err(ArchivePayload::Offload(Box::new(OffloadReturn {
                    state: ReturnState::Admission {
                        input: ready,
                        failure,
                    },
                })))
            }
        }
    }));
    let (ready, installed) = match admitted {
        Ok(Ok(pair)) => pair,
        Ok(Err(payload)) => return return_owned(slot, ticket, Some(custody), payload, terminal),
        Err(_) => {
            terminal.report.get_or_insert_with(|| json!({}))[phase] = json!({"state":"unknown"});
            return return_owned(
                slot,
                ticket,
                Some(custody),
                ArchivePayload::EarlyUnknown,
                terminal,
            );
        }
    };
    let (admitted, outside) = installed.into_parts();
    let entrance = if !Arc::ptr_eq(admitted.journal_root(), &ticket.0.root)
        || !admitted
            .prepared_operation()
            .owned_clock()
            .is_some_and(|clock| Arc::ptr_eq(clock, &ticket.0.clock))
        || !Arc::ptr_eq(admitted.carrier(), ready.carrier())
        || !Arc::ptr_eq(admitted.carrier(), outside.carrier())
        || !Arc::ptr_eq(admitted.journal_root(), outside.journal_root())
        || !admitted
            .prepared_operation()
            .owned_clock()
            .is_some_and(|clock| Arc::ptr_eq(clock, outside.owned_clock()))
    {
        Err("failed")
    } else {
        entry_current(slot, &ticket)
    };
    if let Err(reason) = entrance {
        terminal.reason_code = Some(reason.into());
        return return_owned(
            slot,
            ticket,
            Some(custody),
            ArchivePayload::Offload(Box::new(OffloadReturn {
                state: ReturnState::Entry {
                    input: ready,
                    admitted: Box::new(admitted.into_owned_hold()),
                    outside,
                },
            })),
            terminal,
        );
    }
    #[cfg(all(test, feature = "full-tests", target_os = "linux"))]
    tests::admitted();
    // The outside finish owner survives actor and owning-settlement unwind.
    let actor = catch_unwind(AssertUnwindSafe(|| match ready {
        Inputs::Selected { ready, today, now } => {
            let actor = run::create_owned_offload_offers(admitted, *ready, today, now);
            project(&mut terminal, offload_report::creation(&actor));
            #[cfg(all(test, feature = "full-tests", target_os = "linux"))]
            tests::after_actor();
            match actor.finish() {
                OwnedOffloadCreationFinish::Held(hold) => {
                    terminal.report = Some(offload_report::creation_hold(&hold));
                    Err(ActorHold::Selected(hold))
                }
                OwnedOffloadCreationFinish::Settled(settled) => {
                    let (read, report) = settled.into_parts();
                    terminal.report = Some(offload_report::creation_settled(&report));
                    Ok((read, SettledReport::Selected(Box::new(report))))
                }
            }
        }
        Inputs::Planned(ready) => {
            let actor = run::create_owned_planned_offload_offers(admitted, *ready);
            project(&mut terminal, offload_report::plan(&actor));
            #[cfg(all(test, feature = "full-tests", target_os = "linux"))]
            tests::after_actor();
            match actor.finish() {
                OwnedOffloadPlanFinish::Held(hold) => {
                    terminal.report = Some(offload_report::plan_hold(&hold));
                    Err(ActorHold::Planned(hold))
                }
                OwnedOffloadPlanFinish::Settled(settled) => {
                    let (read, report) = settled.into_parts();
                    terminal.report = Some(offload_report::plan_settled(&report));
                    Ok((read, SettledReport::Planned(Box::new(report))))
                }
            }
        }
    }));
    let state = match actor {
        Ok(Err(actor)) => {
            terminal.report.get_or_insert_with(|| json!({}))["settlement"] =
                json!({"state":"held"});
            ReturnState::Held { actor, outside }
        }
        Ok(Ok((read, report))) => {
            terminal.report.get_or_insert_with(|| json!({}))["settlement"] =
                json!({"state":"settled"});
            // Genuine settled report remains outside consuming Runtime finish.
            match catch_unwind(AssertUnwindSafe(|| outside.finish_owned(read))) {
                Ok(runtime) => {
                    terminal.report.as_mut().expect("actual report")["runtime"] =
                        runtime_facts(&runtime);
                    if runtime.state() == DirectReadFinishState::Completed {
                        terminal.phase = "done".into();
                        terminal.reason_code = None;
                    }
                    ReturnState::Finished { report, runtime }
                }
                Err(_) => {
                    terminal.report.as_mut().expect("actual report")["runtime"] =
                        json!({"state":"unknown"});
                    ReturnState::FinishUnknown { report }
                }
            }
        }
        Err(_) => {
            let runtime = outside.into_unknown();
            let report = terminal.report.get_or_insert_with(|| json!({}));
            report["settlement"] = json!({"state":"unknown"});
            report["runtime"] = runtime_facts(&runtime);
            ReturnState::ActorUnknown { runtime }
        }
    };
    return_owned(
        slot,
        ticket,
        Some(custody),
        ArchivePayload::Offload(Box::new(OffloadReturn { state })),
        terminal,
    );
}

fn project(terminal: &mut Terminal, report: serde_json::Value) {
    terminal.report = Some(report);
}

#[cfg(all(test, feature = "full-tests"))]
#[cfg(target_os = "linux")]
mod tests;
