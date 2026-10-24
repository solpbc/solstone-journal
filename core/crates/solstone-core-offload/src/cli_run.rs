// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One installed CLI operation, retained through its actual print/exit boundary.

use std::io;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use solstone_core_backup::archive_activity::{
    ArchivePreparationCustody, OwnedArchivePreparationFailure, OwnedArchivePreparationInput,
    OwnedPreparedArchiveHold,
};
use solstone_core_backup::archive_admission::{
    AdmissionError, ArchiveBudget, ArchiveConfiguration, Clock,
};
use solstone_core_backup_runtime::{
    ClosedToolError, DirectReadFinishOwner, DirectReadFinishState, OwnedArchiveAdmissionFailure,
    OwnedDirectReadFinish, UreqHttpTransport, admit_prepared_archive_owned_creator_installed,
    resolve_rclone_member, resolve_restic_member,
};
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::{JournalRoot, JournalRootError};
use solstone_core_retention::remove_marked::recover_interrupted_removals;
use solstone_core_retention::restore_intent::recover_abandoned_restores;

use crate::run::{
    OwnedOffloadPlanFinish, OwnedOffloadPlanHold, OwnedOffloadPlanInputFailure,
    OwnedOffloadPlanInputs, SettledOffloadPlanReport, create_owned_planned_offload_offers,
    prepare_owned_offload_plan,
};

mod report;
pub use report::{
    CliArchiveFact, CliCopyFact, CliCreationFacts, CliLedgerFact, CliMutationFact,
    CliObservedEffects, CliReconciliationFact,
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliUnwindPhase {
    Preparation,
    Preflight,
    Admission,
    Actor,
    PlanFinish,
    RuntimeFinish,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliRunPhase {
    Prelaunch,
    Skipped,
    PreparationRefused,
    PreparedHeld,
    AdmissionRefused,
    PlanHeld,
    Finished,
    Unknown(CliUnwindPhase),
}

pub enum CliPrelaunchRefusal {
    DryRun,
    UnsupportedPlatform,
    NamespaceCooperationHeld,
    Root(JournalRootError),
    Budget(AdmissionError),
}

pub enum CliPreparedRefusal {
    Timestamp,
    Operation(AdmissionError),
    Custody(io::Error),
    Planning(Box<OwnedOffloadPlanInputFailure>),
    Executable(io::Error),
    Member(ClosedToolError),
}

enum Custody {
    Prelaunch {
        refusal: CliPrelaunchRefusal,
        _root: Option<Arc<JournalRoot>>,
        _clock: Arc<dyn Clock + Send + Sync>,
    },
    /// Offload is off in this journal's configuration, so nothing was prepared.
    Skipped {
        _root: Arc<JournalRoot>,
        _clock: Arc<dyn Clock + Send + Sync>,
    },
    PreparationFailure {
        failure: Box<OwnedArchivePreparationFailure>,
        custody: Option<ArchivePreparationCustody>,
    },
    PreparedHold {
        prepared: Box<OwnedPreparedArchiveHold>,
        refusal: CliPreparedRefusal,
        readiness: Option<Box<OwnedOffloadPlanInputs>>,
        custody: ArchivePreparationCustody,
    },
    AdmissionFailure {
        failure: Box<OwnedArchiveAdmissionFailure>,
        readiness: Box<OwnedOffloadPlanInputs>,
        custody: ArchivePreparationCustody,
    },
    PlanHold {
        plan: Box<OwnedOffloadPlanHold>,
        outside: DirectReadFinishOwner,
        custody: ArchivePreparationCustody,
    },
    Finished {
        report: Box<SettledOffloadPlanReport>,
        runtime: OwnedDirectReadFinish,
        custody: ArchivePreparationCustody,
    },
    Unknown {
        phase: CliUnwindPhase,
        prepared: Option<Box<OwnedPreparedArchiveHold>>,
        readiness: Option<Box<OwnedOffloadPlanInputs>>,
        runtime: Option<OwnedDirectReadFinish>,
        report: Option<Box<SettledOffloadPlanReport>>,
        custody: ArchivePreparationCustody,
    },
}

/// Borrow output while retaining this complete concrete result through exit.
#[must_use = "Retain the whole offload result through the command receiving boundary"]
pub struct OwnedOffloadCliRun {
    custody: Custody,
    observed: CliObservedEffects,
    binding_errors: Vec<io::Error>,
    stdout: String,
    stderr: String,
    exit_code: i32,
}

impl std::fmt::Debug for OwnedOffloadCliRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedOffloadCliRun")
            .field("phase", &self.phase())
            .field("exit_code", &self.exit_code)
            .finish_non_exhaustive()
    }
}

impl OwnedOffloadCliRun {
    pub fn output(&self) -> (&str, &str, i32) {
        (&self.stdout, &self.stderr, self.exit_code)
    }
    pub fn observed(&self) -> &CliObservedEffects {
        &self.observed
    }
    pub fn binding_errors(&self) -> &[io::Error] {
        &self.binding_errors
    }
    pub fn phase(&self) -> CliRunPhase {
        match &self.custody {
            Custody::Prelaunch { .. } => CliRunPhase::Prelaunch,
            Custody::Skipped { .. } => CliRunPhase::Skipped,
            Custody::PreparationFailure { .. } => CliRunPhase::PreparationRefused,
            Custody::PreparedHold { .. } => CliRunPhase::PreparedHeld,
            Custody::AdmissionFailure { .. } => CliRunPhase::AdmissionRefused,
            Custody::PlanHold { .. } => CliRunPhase::PlanHeld,
            Custody::Finished { .. } => CliRunPhase::Finished,
            Custody::Unknown { phase, .. } => CliRunPhase::Unknown(*phase),
        }
    }
    pub fn preparation_custody(&self) -> Option<&ArchivePreparationCustody> {
        match &self.custody {
            Custody::Prelaunch { .. } | Custody::Skipped { .. } => None,
            Custody::PreparationFailure { custody, .. } => custody.as_ref(),
            Custody::PreparedHold { custody, .. }
            | Custody::AdmissionFailure { custody, .. }
            | Custody::PlanHold { custody, .. }
            | Custody::Finished { custody, .. }
            | Custody::Unknown { custody, .. } => Some(custody),
        }
    }
    pub fn prelaunch_refusal(&self) -> Option<&CliPrelaunchRefusal> {
        match &self.custody {
            Custody::Prelaunch { refusal, .. } => Some(refusal),
            _ => None,
        }
    }
    pub fn preparation_failure(&self) -> Option<&OwnedArchivePreparationFailure> {
        match &self.custody {
            Custody::PreparationFailure { failure, .. } => Some(failure),
            _ => None,
        }
    }
    pub fn prepared_hold(&self) -> Option<&OwnedPreparedArchiveHold> {
        match &self.custody {
            Custody::PreparedHold { prepared, .. } => Some(prepared),
            Custody::Unknown { prepared, .. } => prepared.as_deref(),
            _ => None,
        }
    }
    pub fn prepared_refusal(&self) -> Option<&CliPreparedRefusal> {
        match &self.custody {
            Custody::PreparedHold { refusal, .. } => Some(refusal),
            _ => None,
        }
    }
    pub fn ready_inputs(&self) -> Option<&OwnedOffloadPlanInputs> {
        match &self.custody {
            Custody::PreparedHold { readiness, .. } | Custody::Unknown { readiness, .. } => {
                readiness.as_deref()
            }
            Custody::AdmissionFailure { readiness, .. } => Some(readiness),
            _ => None,
        }
    }
    pub fn admission_failure(&self) -> Option<&OwnedArchiveAdmissionFailure> {
        match &self.custody {
            Custody::AdmissionFailure { failure, .. } => Some(failure),
            _ => None,
        }
    }
    pub fn plan_hold(&self) -> Option<&OwnedOffloadPlanHold> {
        match &self.custody {
            Custody::PlanHold { plan, .. } => Some(plan),
            _ => None,
        }
    }
    pub fn outside_owner(&self) -> Option<&DirectReadFinishOwner> {
        match &self.custody {
            Custody::PlanHold { outside, .. } => Some(outside),
            _ => None,
        }
    }
    pub fn settled_report(&self) -> Option<&SettledOffloadPlanReport> {
        match &self.custody {
            Custody::Finished { report, .. } => Some(report),
            Custody::Unknown { report, .. } => report.as_deref(),
            _ => None,
        }
    }
    pub fn runtime(&self) -> Option<&OwnedDirectReadFinish> {
        match &self.custody {
            Custody::Finished { runtime, .. } => Some(runtime),
            Custody::Unknown { runtime, .. } => runtime.as_ref(),
            _ => None,
        }
    }

    fn receive(custody: Custody, observed: CliObservedEffects) -> Self {
        let mut run = Self {
            custody,
            observed,
            binding_errors: Vec::new(),
            stdout: String::new(),
            stderr: String::new(),
            exit_code: 1,
        };
        if let Some(custody) = run.preparation_custody()
            && let Err(error) = custody.revalidate()
        {
            run.binding_errors.push(error);
        }
        run.render();
        if let Some(custody) = run.preparation_custody()
            && let Err(error) = custody.revalidate()
        {
            run.binding_errors.push(error);
            run.render();
        }
        run
    }
}

/// Whether this host runs the installed offload producer. Elsewhere every
/// non-preview run is refused before it opens the journal.
pub const PRODUCER_SUPPORTED: bool = cfg!(any(
    target_os = "linux",
    target_os = "macos",
    target_os = "windows"
));

/// Run the installed offload producer while retaining its complete outcome.
pub fn run_cli_offload(
    journal: &Path,
    clock: Arc<dyn Clock + Send + Sync>,
    dry_run: bool,
) -> OwnedOffloadCliRun {
    let refusal = if dry_run {
        Some(CliPrelaunchRefusal::DryRun)
    } else if !PRODUCER_SUPPORTED {
        Some(CliPrelaunchRefusal::UnsupportedPlatform)
    } else {
        None
    };
    if let Some(refusal) = refusal {
        return OwnedOffloadCliRun::receive(
            Custody::Prelaunch {
                refusal,
                _root: None,
                _clock: clock,
            },
            CliObservedEffects::default(),
        );
    }
    let root = match JournalRoot::open(journal) {
        Ok(root) => Arc::new(root),
        Err(error) => {
            return OwnedOffloadCliRun::receive(
                Custody::Prelaunch {
                    refusal: CliPrelaunchRefusal::Root(error),
                    _root: None,
                    _clock: clock,
                },
                CliObservedEffects::default(),
            );
        }
    };
    // A run, removal or restore interrupted by a crash would refuse this
    // admission. Recovery may hash originals, so it runs before the deadline starts.
    if let Ok(activity) = ActivityLease::try_acquire(Arc::clone(&root)) {
        recover_interrupted_removals(&activity);
        recover_abandoned_restores(&activity);
    }
    // Offload stays off until the owner turns it on, and a scheduled run then
    // has nothing to do. Configuration that can't be read goes on to
    // preparation, which refuses it.
    if ArchiveConfiguration::capture(&root).is_ok_and(|config| !config.offload_enabled()) {
        return OwnedOffloadCliRun::receive(
            Custody::Skipped {
                _root: root,
                _clock: clock,
            },
            CliObservedEffects::default(),
        );
    }
    let budget = match ArchiveBudget::new(clock.as_ref()) {
        Ok(budget) => budget,
        Err(error) => {
            return OwnedOffloadCliRun::receive(
                Custody::Prelaunch {
                    refusal: CliPrelaunchRefusal::Budget(error),
                    _root: Some(root),
                    _clock: clock,
                },
                CliObservedEffects::default(),
            );
        }
    };
    let (input, custody) = match OwnedArchivePreparationInput::capture(root, budget, clock) {
        Ok(pair) => pair,
        Err(failure) => {
            return OwnedOffloadCliRun::receive(
                Custody::PreparationFailure {
                    failure: Box::new(failure),
                    custody: None,
                },
                CliObservedEffects::default(),
            );
        }
    };
    let prepared = match catch_unwind(AssertUnwindSafe(|| {
        test_cut(CliUnwindPhase::Preparation);
        input.prepare()
    })) {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(failure)) => {
            return OwnedOffloadCliRun::receive(
                Custody::PreparationFailure {
                    failure: Box::new(failure),
                    custody: Some(custody),
                },
                CliObservedEffects::default(),
            );
        }
        Err(_) => {
            return unknown(
                CliUnwindPhase::Preparation,
                custody,
                None,
                None,
                None,
                None,
                CliObservedEffects::default(),
            );
        }
    };
    let Some(at) = DateTime::<Utc>::from_timestamp(prepared.owned_clock().now_unix(), 0) else {
        return OwnedOffloadCliRun::receive(
            Custody::PreparedHold {
                prepared: Box::new(prepared.into_owned_hold()),
                refusal: CliPreparedRefusal::Timestamp,
                readiness: None,
                custody,
            },
            CliObservedEffects::default(),
        );
    };
    let readiness = match catch_unwind(AssertUnwindSafe(|| {
        test_cut(CliUnwindPhase::Preflight);
        prepare_owned_offload_plan(&prepared, at.fixed_offset())
    })) {
        Ok(Ok(readiness)) => readiness,
        Ok(Err(failure)) => {
            let observed = CliObservedEffects::planning(failure.planning());
            return OwnedOffloadCliRun::receive(
                Custody::PreparedHold {
                    prepared: Box::new(prepared.into_owned_hold()),
                    refusal: CliPreparedRefusal::Planning(Box::new(failure)),
                    readiness: None,
                    custody,
                },
                observed,
            );
        }
        Err(_) => {
            return unknown(
                CliUnwindPhase::Preflight,
                custody,
                Some(Box::new(prepared.into_owned_hold())),
                None,
                None,
                None,
                CliObservedEffects::default(),
            );
        }
    };
    let observed = CliObservedEffects::planning(readiness.planning());
    let executable = match std::env::current_exe() {
        Ok(executable) => executable,
        Err(error) => {
            return OwnedOffloadCliRun::receive(
                Custody::PreparedHold {
                    prepared: Box::new(prepared.into_owned_hold()),
                    refusal: CliPreparedRefusal::Executable(error),
                    readiness: Some(Box::new(readiness)),
                    custody,
                },
                observed,
            );
        }
    };
    let operated = prepared.configuration().binding().is_some();
    let tools = resolve_restic_member(&executable).and_then(|restic| {
        let rclone = if operated {
            Some(resolve_rclone_member(&executable)?)
        } else {
            None
        };
        Ok((restic, rclone))
    });
    let (restic, rclone) = match tools {
        Ok(tools) => tools,
        Err(error) => {
            return OwnedOffloadCliRun::receive(
                Custody::PreparedHold {
                    prepared: Box::new(prepared.into_owned_hold()),
                    refusal: CliPreparedRefusal::Member(error),
                    readiness: Some(Box::new(readiness)),
                    custody,
                },
                observed,
            );
        }
    };
    let refusal = match custody.revalidate() {
        Err(error) => Some(CliPreparedRefusal::Custody(error)),
        Ok(()) => prepared
            .revalidate()
            .err()
            .map(CliPreparedRefusal::Operation),
    };
    if let Some(refusal) = refusal {
        return OwnedOffloadCliRun::receive(
            Custody::PreparedHold {
                prepared: Box::new(prepared.into_owned_hold()),
                refusal,
                readiness: Some(Box::new(readiness)),
                custody,
            },
            observed,
        );
    }
    let creator = match catch_unwind(AssertUnwindSafe(|| {
        test_cut(CliUnwindPhase::Admission);
        admit_prepared_archive_owned_creator_installed(
            prepared,
            &restic,
            rclone.as_deref(),
            env!("CARGO_PKG_VERSION"),
            &UreqHttpTransport,
        )
    })) {
        Ok(Ok(creator)) => creator,
        Ok(Err(failure)) => {
            return OwnedOffloadCliRun::receive(
                Custody::AdmissionFailure {
                    failure: Box::new(failure),
                    readiness: Box::new(readiness),
                    custody,
                },
                observed,
            );
        }
        Err(_) => {
            return unknown(
                CliUnwindPhase::Admission,
                custody,
                None,
                Some(Box::new(readiness)),
                None,
                None,
                observed,
            );
        }
    };
    let (admitted, outside) = creator.into_parts();
    let plan = match catch_unwind(AssertUnwindSafe(|| {
        test_cut(CliUnwindPhase::Actor);
        create_owned_planned_offload_offers(admitted, readiness)
    })) {
        Ok(plan) => plan,
        Err(_) => {
            return unknown(
                CliUnwindPhase::Actor,
                custody,
                None,
                None,
                Some(outside.into_unknown()),
                None,
                observed,
            );
        }
    };
    let observed = CliObservedEffects::active(&plan);
    let finish = catch_unwind(AssertUnwindSafe(|| {
        test_cut(CliUnwindPhase::PlanFinish);
        plan.finish()
    }));
    match finish {
        Ok(OwnedOffloadPlanFinish::Held(plan)) => {
            let observed = CliObservedEffects::held(&plan);
            OwnedOffloadCliRun::receive(
                Custody::PlanHold {
                    plan,
                    outside,
                    custody,
                },
                observed,
            )
        }
        Ok(OwnedOffloadPlanFinish::Settled(settled)) => {
            let (read, report) = settled.into_parts();
            let observed = CliObservedEffects::settled(&report);
            // The actual report is outside the consuming finish catch.
            let finish = catch_unwind(AssertUnwindSafe(|| {
                test_cut(CliUnwindPhase::RuntimeFinish);
                outside.finish_owned(read)
            }));
            match finish {
                Ok(runtime) => OwnedOffloadCliRun::receive(
                    Custody::Finished {
                        report: Box::new(report),
                        runtime,
                        custody,
                    },
                    observed,
                ),
                Err(_) => unknown(
                    CliUnwindPhase::RuntimeFinish,
                    custody,
                    None,
                    None,
                    None,
                    Some(Box::new(report)),
                    observed,
                ),
            }
        }
        Err(_) => unknown(
            CliUnwindPhase::PlanFinish,
            custody,
            None,
            None,
            Some(outside.into_unknown()),
            None,
            observed,
        ),
    }
}

fn unknown(
    phase: CliUnwindPhase,
    custody: ArchivePreparationCustody,
    prepared: Option<Box<OwnedPreparedArchiveHold>>,
    readiness: Option<Box<OwnedOffloadPlanInputs>>,
    runtime: Option<OwnedDirectReadFinish>,
    report: Option<Box<SettledOffloadPlanReport>>,
    observed: CliObservedEffects,
) -> OwnedOffloadCliRun {
    OwnedOffloadCliRun::receive(
        Custody::Unknown {
            phase,
            prepared,
            readiness,
            runtime,
            report,
            custody,
        },
        observed,
    )
}

#[cfg(all(test, feature = "full-tests"))]
thread_local! {
    #[cfg(target_os = "android")]
    static PANIC_AT: Box<std::cell::Cell<Option<CliUnwindPhase>>> = Box::new(std::cell::Cell::new(None));
    #[cfg(not(target_os = "android"))]
    static PANIC_AT: std::cell::Cell<Option<CliUnwindPhase>> = const { std::cell::Cell::new(None) };
}

fn test_cut(phase: CliUnwindPhase) {
    #[cfg(all(test, feature = "full-tests", target_os = "linux"))]
    packaged::observe(phase);
    #[cfg(all(test, feature = "full-tests"))]
    PANIC_AT.with(|cut| {
        if cut.get() == Some(phase) {
            cut.set(None);
            panic!("CLI consuming cut: {phase:?}");
        }
    });
    #[cfg(not(all(test, feature = "full-tests")))]
    let _ = phase;
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "full-tests", target_os = "linux"))]
mod packaged;
