// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! One installed CLI restore, retained through its actual print/exit boundary.
//!
//! The same admission, read and owning restore the backup page runs, with the
//! command itself as the outside owner: a held result stays alive until exit.

use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

use solstone_core_backup::archive_activity::{
    ArchivePreparationCustody, OwnedArchivePreparationFailure, OwnedArchivePreparationInput,
    OwnedPreparedArchiveHold,
};
use solstone_core_backup::archive_admission::{
    AdmissionError, ArchiveBudget, ArchiveCancellation, Clock,
};
use solstone_core_backup_runtime::{
    ArchiveError, ClosedToolError, DirectReadFinishOwner, DirectReadFinishState,
    OwnedArchiveAdmissionFailure, OwnedDirectReadFinish, UreqHttpTransport,
    admit_prepared_archive_owned_current_read_installed,
    admit_prepared_archive_owned_read_installed, resolve_restic_member,
};
use solstone_core_journal_io::JournalRoot;
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::activity_obligations::observe_obligations;
use solstone_core_journal_io::whole_journal_restore::{
    TargetUse, begin_target_use_bound, whole_journal_restore_blocks,
};

mod local_cleanup;
use local_cleanup::{CompletedLocalCleanup, LocalCleanup};

use crate::restore::owned::{
    OwnedRestoreFinishHold, RestoreFinishRefusal, SettledRestoreReport, restore_owned_all,
    restore_owned_day,
};
use crate::restore::{OFFLOAD_RESTORE_REASONS, RestoreResult, selected_repository};

/// How far the owning restore got, independent of the restore's own status.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CliRestoreSettlement {
    /// No archive read or media restore started. Local recovery may have completed.
    NotStarted,
    /// Every file settled and the read finished.
    Settled,
    /// A refused restore released its own stages and reaped its children.
    Refused,
    /// Stages or children remain; the original owners are retained.
    Held,
    /// An owner unwound; its effects are not known.
    Unknown,
}

enum Custody {
    LocalCleanup {
        _owner: Box<LocalCleanup>,
    },
    NotStarted {
        _root: Option<Arc<JournalRoot>>,
    },
    PreparationFailure {
        _failure: Box<OwnedArchivePreparationFailure>,
        _custody: Option<ArchivePreparationCustody>,
    },
    PreparedHold {
        _prepared: Box<OwnedPreparedArchiveHold>,
        _custody: ArchivePreparationCustody,
    },
    AdmissionFailure {
        _failure: Box<OwnedArchiveAdmissionFailure>,
        _custody: ArchivePreparationCustody,
    },
    Held {
        _native: Box<OwnedRestoreFinishHold>,
        _outside: DirectReadFinishOwner,
        _custody: ArchivePreparationCustody,
    },
    Finished {
        _report: Box<SettledRestoreReport>,
        runtime: OwnedDirectReadFinish,
        _custody: ArchivePreparationCustody,
    },
    Unknown {
        runtime: Option<OwnedDirectReadFinish>,
        _report: Option<Box<SettledRestoreReport>>,
        _custody: Option<ArchivePreparationCustody>,
    },
}

/// Borrow the result while retaining every original owner through exit.
#[must_use = "Retain the whole restore result through the command receiving boundary"]
pub struct OwnedRestoreCliRun {
    // Native owners retire before their original whole-restore admission.
    custody: Custody,
    _local_cleanup: Option<CompletedLocalCleanup>,
    _target_use: Option<TargetUse>,
    settlement: CliRestoreSettlement,
    result: RestoreResult,
}

impl std::fmt::Debug for OwnedRestoreCliRun {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("OwnedRestoreCliRun")
            .field("settlement", &self.settlement)
            .field("status", &self.result.status)
            .finish_non_exhaustive()
    }
}

impl OwnedRestoreCliRun {
    #[cfg(all(test, feature = "full-tests"))]
    pub(crate) fn local_native_removals(
        &self,
    ) -> Vec<&solstone_core_journal_io::media::MediaRemoval> {
        match &self.custody {
            Custody::LocalCleanup { _owner } => _owner.removals().collect(),
            _ => vec![],
        }
    }

    #[cfg(all(test, feature = "full-tests"))]
    pub(crate) fn local_native_creations(
        &self,
    ) -> Vec<&solstone_core_journal_io::media::MediaStageCreation> {
        match &self.custody {
            Custody::LocalCleanup { _owner } => _owner.creations().collect(),
            _ => vec![],
        }
    }

    pub fn result(&self) -> &RestoreResult {
        &self.result
    }
    pub fn settlement(&self) -> CliRestoreSettlement {
        self.settlement
    }
    /// The read's finish, once the actor returned an owner that could finish.
    pub fn runtime(&self) -> Option<DirectReadFinishState> {
        match &self.custody {
            Custody::Finished { runtime, .. } => Some(runtime.state()),
            Custody::Unknown { runtime, .. } => runtime.as_ref().map(|runtime| runtime.state()),
            _ => None,
        }
    }
    /// True only when the restore and its read both finished cleanly.
    pub fn completed(&self) -> bool {
        self.settlement == CliRestoreSettlement::Settled
            && self.runtime() == Some(DirectReadFinishState::Completed)
            && matches!(self.result.status.as_str(), "ok" | "no_op")
    }

    fn new(custody: Custody, settlement: CliRestoreSettlement, result: RestoreResult) -> Self {
        Self {
            custody,
            _local_cleanup: None,
            _target_use: None,
            settlement,
            result,
        }
    }
}

fn base(status: &str, reason: &str, day: Option<&str>) -> RestoreResult {
    RestoreResult {
        status: status.into(),
        reason: Some(reason.into()),
        scope: if day.is_some() { "day" } else { "all" }.into(),
        day: day.map(str::to_owned),
        segments_selected: 0,
        segments_restored: 0,
        files_expected: 0,
        files_restored: 0,
        bytes_expected: 0,
        bytes_restored: 0,
        reason_detail: None,
        details: vec![],
    }
}

fn with_detail(mut result: RestoreResult, detail: String) -> RestoreResult {
    result.reason_detail = Some(detail);
    result
}

fn with_settlement_detail(
    mut result: RestoreResult,
    refusal: RestoreFinishRefusal<'_>,
) -> RestoreResult {
    let detail = match refusal {
        RestoreFinishRefusal::Native {
            file,
            reason,
            source,
            readback,
        } => format!(
            "settlement native file={file:?} reason={reason} source={source:?} readback={readback:?}"
        ),
        RestoreFinishRefusal::Lower(reason) => format!("settlement readback {reason:?}"),
    };
    match &mut result.reason_detail {
        Some(existing) => {
            existing.push_str("; ");
            existing.push_str(&detail);
        }
        None => result.reason_detail = Some(detail),
    }
    result
}

fn admission_code(error: AdmissionError) -> &'static str {
    match error {
        AdmissionError::ConfigurationUnavailable => "configuration_unavailable",
        AdmissionError::ConfigurationChanged => "configuration_changed",
        AdmissionError::JournalIdentityChanged => "journal_identity_changed",
        AdmissionError::DeadlineExceeded => "deadline_exceeded",
        AdmissionError::ClockChanged => "clock_changed",
        AdmissionError::CredentialsExpired => "credentials_expired",
        AdmissionError::Cancelled => "cancelled",
    }
}

fn archive_reason(error: &ArchiveError) -> &'static str {
    match error {
        ArchiveError::NotConfigured => "backup_not_ready",
        ArchiveError::ToolUnavailable => "restic_unavailable",
        ArchiveError::ToolFailed { reason } => OFFLOAD_RESTORE_REASONS
            .into_iter()
            .find(|known| known == reason)
            .unwrap_or("destination_admission_failed"),
        _ => "destination_admission_failed",
    }
}

fn tool_reason(error: &ClosedToolError) -> &'static str {
    let code = error.to_string();
    OFFLOAD_RESTORE_REASONS
        .into_iter()
        .find(|known| *known == code)
        .unwrap_or("restic_unavailable")
}

/// Restore offloaded media for one day, or everything, owning the whole run.
pub fn run_cli_restore(
    journal: &Path,
    clock: Arc<dyn Clock + Send + Sync>,
    day: Option<&str>,
) -> OwnedRestoreCliRun {
    run_cli_restore_with_cancellation(journal, clock, day, ArchiveCancellation::new())
}

pub(crate) fn run_cli_restore_with_cancellation(
    journal: &Path,
    clock: Arc<dyn Clock + Send + Sync>,
    day: Option<&str>,
    cancellation: ArchiveCancellation,
) -> OwnedRestoreCliRun {
    let not_started = |root: Option<Arc<JournalRoot>>, result: RestoreResult| {
        OwnedRestoreCliRun::new(
            Custody::NotStarted { _root: root },
            CliRestoreSettlement::NotStarted,
            result,
        )
    };
    if !cfg!(any(
        target_os = "linux",
        target_os = "macos",
        target_os = "windows"
    )) {
        return not_started(None, base("refused", "restore_held", day));
    }
    let root = match JournalRoot::open(journal) {
        Ok(root) => Arc::new(root),
        Err(error) => {
            return not_started(
                None,
                with_detail(base("refused", "segment_identity", day), error.to_string()),
            );
        }
    };
    let target_use = match begin_target_use_bound(Arc::clone(&root)) {
        Ok(guard) => guard,
        Err(error) => {
            return not_started(
                Some(root),
                with_detail(base("refused", "restore_held", day), error.to_string()),
            );
        }
    };
    let mut run = run_captured_root(root, clock, day, cancellation);
    run._target_use = Some(target_use);
    run
}

fn run_captured_root(
    root: Arc<JournalRoot>,
    clock: Arc<dyn Clock + Send + Sync>,
    day: Option<&str>,
    cancellation: ArchiveCancellation,
) -> OwnedRestoreCliRun {
    let not_started = |result: RestoreResult| {
        OwnedRestoreCliRun::new(
            Custody::NotStarted {
                _root: Some(Arc::clone(&root)),
            },
            CliRestoreSettlement::NotStarted,
            result,
        )
    };
    let Ok(now) = u64::try_from(clock.now_unix()) else {
        return not_started(base("error", "failed", day));
    };
    let budget = match ArchiveBudget::new_cancellable(clock.as_ref(), cancellation) {
        Ok(budget) => budget,
        Err(error) => {
            return not_started(with_detail(
                base("refused", "restore_held", day),
                admission_code(error).into(),
            ));
        }
    };
    let checkpoint = || {
        root.revalidate_canonical_binding()
            .map_err(std::io::Error::other)?;
        if whole_journal_restore_blocks(root.canonical_path()) {
            return Err(std::io::Error::other("whole-journal restore is pending"));
        }
        budget
            .remaining(clock.as_ref())
            .map_err(|error| std::io::Error::other(admission_code(error)))?;
        Ok(())
    };
    if let Err(error) = checkpoint() {
        return not_started(with_detail(
            base("refused", "restore_held", day),
            error.to_string(),
        ));
    }
    let observed = match observe_obligations(&root) {
        Ok(observed) => observed,
        Err(error) => {
            return not_started(with_detail(
                base("refused", "restore_held", day),
                error.to_string(),
            ));
        }
    };
    let completed = if observed.entries().is_empty() {
        CompletedLocalCleanup::default()
    } else {
        // The first possible local mutation is ordinary lock admission.
        if let Err(error) = checkpoint() {
            return not_started(with_detail(
                base("refused", "restore_held", day),
                error.to_string(),
            ));
        }
        let activity = match ActivityLease::try_acquire(Arc::clone(&root)) {
            Ok(activity) => activity,
            Err(error) => {
                return not_started(with_detail(
                    base("refused", "restore_held", day),
                    error.to_string(),
                ));
            }
        };
        let mut cleanup = LocalCleanup::new(activity);
        let result = catch_unwind(AssertUnwindSafe(|| {
            cleanup.prepare(&checkpoint)?;
            cleanup.retire(&checkpoint)
        }));
        match result {
            Ok(Ok(())) => match cleanup.into_completed() {
                Ok(report) => report,
                Err(owner) => {
                    return OwnedRestoreCliRun::new(
                        Custody::LocalCleanup { _owner: owner },
                        CliRestoreSettlement::Held,
                        base("refused", "restore_held", day),
                    );
                }
            },
            Ok(Err(error)) => {
                return OwnedRestoreCliRun::new(
                    Custody::LocalCleanup {
                        _owner: Box::new(cleanup),
                    },
                    CliRestoreSettlement::Held,
                    with_detail(base("refused", "restore_held", day), error.to_string()),
                );
            }
            Err(_) => {
                return OwnedRestoreCliRun::new(
                    Custody::LocalCleanup {
                        _owner: Box::new(cleanup),
                    },
                    CliRestoreSettlement::Unknown,
                    with_detail(
                        base("error", "restore_held", day),
                        "local recovery unwound; disposition unknown".into(),
                    ),
                );
            }
        }
    };
    if let Err(error) = checkpoint() {
        let mut run = not_started(with_detail(
            base("refused", "restore_held", day),
            error.to_string(),
        ));
        run._local_cleanup = Some(completed);
        return run;
    }
    let mut run = run_read(root, clock, day, budget, now);
    run._local_cleanup = Some(completed);
    run
}

fn run_read(
    root: Arc<JournalRoot>,
    clock: Arc<dyn Clock + Send + Sync>,
    day: Option<&str>,
    budget: ArchiveBudget,
    now: u64,
) -> OwnedRestoreCliRun {
    let not_started = |root: Option<Arc<JournalRoot>>, result: RestoreResult| {
        OwnedRestoreCliRun::new(
            Custody::NotStarted { _root: root },
            CliRestoreSettlement::NotStarted,
            result,
        )
    };
    let repository = match selected_repository(&root, day) {
        Ok(repository) => repository,
        Err("nothing_to_restore") => {
            return not_started(Some(root), base("no_op", "nothing_to_restore", day));
        }
        Err("ledger_degraded") => {
            return not_started(Some(root), base("error", "ledger_degraded", day));
        }
        Err(reason) => {
            return not_started(
                Some(root),
                with_detail(base("refused", "archive_unbound", day), reason.into()),
            );
        }
    };
    let preparation_refused =
        |failure: OwnedArchivePreparationFailure, custody: Option<ArchivePreparationCustody>| {
            let result = with_detail(
                base("refused", "restore_held", day),
                admission_code(failure.reason()).into(),
            );
            OwnedRestoreCliRun::new(
                Custody::PreparationFailure {
                    _failure: Box::new(failure),
                    _custody: custody,
                },
                CliRestoreSettlement::NotStarted,
                result,
            )
        };
    let (input, custody) =
        match OwnedArchivePreparationInput::capture(Arc::clone(&root), budget, clock) {
            Ok(pair) => pair,
            Err(failure) => return preparation_refused(failure, None),
        };
    let unknown = |custody: Option<ArchivePreparationCustody>,
                   runtime: Option<OwnedDirectReadFinish>,
                   report: Option<Box<SettledRestoreReport>>,
                   result: RestoreResult| {
        OwnedRestoreCliRun::new(
            Custody::Unknown {
                runtime,
                _report: report,
                _custody: custody,
            },
            CliRestoreSettlement::Unknown,
            result,
        )
    };
    let prepared = match catch_unwind(AssertUnwindSafe(|| input.prepare())) {
        Ok(Ok(prepared)) => prepared,
        Ok(Err(failure)) => return preparation_refused(failure, Some(custody)),
        Err(_) => return unknown(Some(custody), None, None, base("error", "failed", day)),
    };
    let restic = match std::env::current_exe()
        .map_err(|error| ClosedToolError::ResticUnavailable {
            detail: error.to_string(),
            guidance: error.to_string(),
        })
        .and_then(|executable| resolve_restic_member(&executable))
    {
        Ok(restic) => restic,
        Err(error) => {
            let result = with_detail(
                base("refused", tool_reason(&error), day),
                error.detail().into(),
            );
            return OwnedRestoreCliRun::new(
                Custody::PreparedHold {
                    _prepared: Box::new(prepared.into_owned_hold()),
                    _custody: custody,
                },
                CliRestoreSettlement::NotStarted,
                result,
            );
        }
    };
    let admitted = catch_unwind(AssertUnwindSafe(|| match repository {
        Some(repository) => admit_prepared_archive_owned_read_installed(
            prepared,
            repository,
            &restic,
            env!("CARGO_PKG_VERSION"),
            &UreqHttpTransport,
        ),
        None => admit_prepared_archive_owned_current_read_installed(
            prepared,
            &restic,
            env!("CARGO_PKG_VERSION"),
            &UreqHttpTransport,
        ),
    }));
    let installed = match admitted {
        Ok(Ok(installed)) => installed,
        Ok(Err(failure)) => {
            let result = with_detail(
                base("refused", archive_reason(&failure.error()), day),
                failure.error().to_string(),
            );
            return OwnedRestoreCliRun::new(
                Custody::AdmissionFailure {
                    _failure: Box::new(failure),
                    _custody: custody,
                },
                CliRestoreSettlement::NotStarted,
                result,
            );
        }
        Err(_) => return unknown(Some(custody), None, None, base("error", "failed", day)),
    };
    let (read, outside) = installed.into_parts();
    // The outside owner never enters the actor's unwind boundary.
    let mut projected = base("error", "failed", day);
    let actor = catch_unwind(AssertUnwindSafe(|| {
        let native = match day {
            Some(day) => restore_owned_day(read, day, now),
            None => restore_owned_all(read, now),
        };
        projected = native.report().clone();
        match native.settle_direct() {
            Ok((settled, report)) => Ok(Ok((settled, report))),
            Err(held) => {
                projected = held.report().clone();
                match (*held).release_refused() {
                    Ok((refused, report)) => Ok(Err((refused, report))),
                    Err(held) => Err(held),
                }
            }
        }
    }));
    match actor {
        Ok(Ok(Ok((settled, report)))) => {
            let result = report.report().clone();
            match catch_unwind(AssertUnwindSafe(|| outside.finish_owned(settled))) {
                Ok(runtime) => OwnedRestoreCliRun::new(
                    Custody::Finished {
                        _report: Box::new(report),
                        runtime,
                        _custody: custody,
                    },
                    CliRestoreSettlement::Settled,
                    result,
                ),
                Err(_) => unknown(Some(custody), None, Some(Box::new(report)), result),
            }
        }
        Ok(Ok(Err((refused, report)))) => {
            let result = report.report().clone();
            match catch_unwind(AssertUnwindSafe(|| outside.finish_refused(refused))) {
                Ok(runtime) => OwnedRestoreCliRun::new(
                    Custody::Finished {
                        _report: Box::new(report),
                        runtime,
                        _custody: custody,
                    },
                    CliRestoreSettlement::Refused,
                    result,
                ),
                Err(_) => unknown(Some(custody), None, Some(Box::new(report)), result),
            }
        }
        Ok(Err(held)) => {
            let result = with_settlement_detail(projected, held.refusal());
            OwnedRestoreCliRun::new(
                Custody::Held {
                    _native: held,
                    _outside: outside,
                    _custody: custody,
                },
                CliRestoreSettlement::Held,
                result,
            )
        }
        Err(_) => {
            let runtime = outside.into_unknown();
            unknown(Some(custody), Some(runtime), None, projected)
        }
    }
}

#[cfg(test)]
mod tests;

#[cfg(all(test, feature = "full-tests"))]
#[cfg(any(target_os = "linux", target_os = "macos"))]
mod packaged_tests;
