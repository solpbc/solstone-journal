// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Consuming typed-removal settlement and closed custody.

use super::*;
use crate::archive_approval::{ArchiveApprovalError, OwnedArchiveApproval};
use solstone_core_backup::archive_readback::{
    ExactArchiveFile, ReadbackError, RefusedDirectRead, SettledDirectRead,
};
#[cfg(any(unix, windows))]
use solstone_core_journal_io::activity_obligations::{
    RecoveryObservationError, RecoveryObservationFailure as Failure,
};
use solstone_core_journal_io::media::{Cleanup, MediaDigest, MediaIdentity, RemovalReceipt};

/// Independent finish refusal. This is diagnostic, never cleanup authority.
pub enum ReviewedArchiveFinishRefusal {
    IncompleteSelection,
    ActorResources,
    Readback(ReadbackError),
    NativeCleanup(Cleanup),
    Changed(std::io::Error),
    Census(solstone_core_journal_io::activity_obligations::RecoveryObservationError),
    Obligations(Vec<solstone_core_journal_io::activity_obligations::NativeObligation>),
}

enum ClosedReview {
    Pending(Box<solstone_core_backup::archive_readback::OwnedDirectReadHold>),
    Approval(Box<OwnedArchiveApproval>),
}

/// Every original resource survives refusal, without an execution entrance.
#[must_use]
pub struct OwnedReviewedArchiveRemoval {
    checked: crate::review_context::CheckedReviewContext,
    execution: ReviewedExecution,
    approval: ClosedReview,
    finish_refusal: Option<ReviewedArchiveFinishRefusal>,
}

impl OwnedReviewedArchiveRemoval {
    pub fn outcome(&self) -> &Outcome {
        &self.execution.outcome
    }
    pub fn refusal(&self) -> Option<&ReviewedRemovalRefusal> {
        self.execution.refusal.as_ref()
    }
    pub fn finish_refusal(&self) -> Option<&ReviewedArchiveFinishRefusal> {
        self.finish_refusal.as_ref()
    }
    pub fn attempts(&self) -> &[ReviewedFileRemoval] {
        &self.execution.attempts
    }
    pub fn stage_failures(
        &self,
    ) -> &[solstone_core_journal_io::media::OwnedMediaFailure<
        Option<solstone_core_journal_io::media::MediaStage>,
    >] {
        &self.execution.stage_failures
    }
    pub fn mutation(
        &self,
    ) -> Option<
        &Result<
            crate::mark_mutation::BoundMutationOutcome,
            crate::mark_mutation::BoundMutationRefusal,
        >,
    > {
        self.execution.mutation.as_ref()
    }
    pub fn journal_root(&self) -> &solstone_core_journal_io::JournalRoot {
        self.checked.root()
    }
    pub fn selected_marks(&self) -> &[Mark] {
        self.checked.marks()
    }
    pub fn archive_journal_root(&self) -> &std::sync::Arc<solstone_core_journal_io::JournalRoot> {
        match &self.approval {
            ClosedReview::Pending(read) => read.journal_root(),
            ClosedReview::Approval(approval) => approval.journal_root(),
        }
    }
    pub fn archive_carrier(
        &self,
    ) -> &std::sync::Arc<solstone_core_journal_io::media::RootActivityCarrier> {
        match &self.approval {
            ClosedReview::Pending(read) => read.carrier(),
            ClosedReview::Approval(approval) => approval.carrier(),
        }
    }
    pub fn archive_expected_marks(&self) -> Option<&[Mark]> {
        match &self.approval {
            ClosedReview::Pending(_) => None,
            ClosedReview::Approval(approval) => Some(approval.expected_marks()),
        }
    }
    pub fn archive_reads(&self) -> impl Iterator<Item = ArchiveReadState<'_>> {
        match &self.approval {
            ClosedReview::Pending(_) => crate::archive_approval::ArchiveReadStates::empty(),
            ClosedReview::Approval(approval) => approval.read_states(),
        }
    }
    pub fn approval_readback_error(&self) -> Option<ReadbackError> {
        let ClosedReview::Approval(approval) = &self.approval else {
            return None;
        };
        approval
            .readback_error()
            .or_else(|| match approval.reason() {
                Some(ArchiveApprovalError::Readback(error)) => Some(*error),
                _ => None,
            })
    }
    pub fn approval_cleanup(&self) -> Option<Cleanup> {
        let ClosedReview::Approval(approval) = &self.approval else {
            return None;
        };
        match approval.reason() {
            Some(ArchiveApprovalError::Cleanup(cleanup)) => Some(*cleanup),
            _ => None,
        }
    }
    pub fn approval_native_error(&self) -> Option<&std::io::Error> {
        let ClosedReview::Approval(approval) = &self.approval else {
            return None;
        };
        match approval.reason() {
            Some(ArchiveApprovalError::Native(error)) => Some(error),
            _ => None,
        }
    }

    /// End a refused removal that changed nothing: no file, mark or stage of
    /// its own remains and every child its read launched was reaped. Only the
    /// empty carrier is left, for the outside finish owner to retire.
    pub fn release_refused(self) -> Result<(RefusedDirectRead, ReviewedArchiveReport), Box<Self>> {
        #[cfg(not(any(unix, windows)))]
        {
            Err(Box::new(self))
        }
        #[cfg(any(unix, windows))]
        {
            let execution = &self.execution;
            if !execution.attempts.is_empty()
                || !execution.stage_failures.is_empty()
                || execution.carrier.is_some()
                || execution.carrier_failure.is_some()
                || execution.mutation.is_some()
                || execution.outcome.removed_paths().next().is_some()
            {
                return Err(Box::new(self));
            }
            let Self {
                checked,
                execution,
                approval,
                finish_refusal,
            } = self;
            let released = match approval {
                ClosedReview::Pending(read) => {
                    (*read).release_refused().map_err(ClosedReview::Pending)
                }
                ClosedReview::Approval(approval) => (*approval)
                    .release_refused()
                    .map_err(ClosedReview::Approval),
            };
            match released {
                Ok(read) => Ok((read, release_report(checked, execution, Vec::new()))),
                Err(approval) => Err(Box::new(Self {
                    checked,
                    execution,
                    approval,
                    finish_refusal,
                })),
            }
        }
    }
}

/// Actual confirmed dump facts after its already-completed sequential disposal.
pub struct ReviewedArchiveReadReport {
    file: ExactArchiveFile,
    target_identity: MediaIdentity,
    digest: MediaDigest,
    cleanup: Option<Cleanup>,
}
impl ReviewedArchiveReadReport {
    pub fn file(&self) -> &ExactArchiveFile {
        &self.file
    }
    pub fn target_identity(&self) -> &MediaIdentity {
        &self.target_identity
    }
    pub fn digest(&self) -> &MediaDigest {
        &self.digest
    }
    pub fn cleanup(&self) -> Option<Cleanup> {
        self.cleanup
    }
    pub(crate) fn from_actual(
        file: ExactArchiveFile,
        target_identity: MediaIdentity,
        digest: MediaDigest,
        cleanup: Option<Cleanup>,
    ) -> Self {
        Self {
            file,
            target_identity,
            digest,
            cleanup,
        }
    }
}

/// Original physical receipts, with no retained file or activity handles.
pub struct ReviewedRemovalFileReport {
    relative: String,
    class: RemovalClass,
    native: RemovalReceipt,
    #[cfg(unix)]
    append: Option<solstone_core_journal_io::append::BoundAppendReceipt>,
    #[cfg(unix)]
    marker: Option<
        Result<
            solstone_core_journal_io::health_marker::BoundStreamMarkerPublication,
            solstone_core_journal_io::health_marker::BoundStreamMarkerError,
        >,
    >,
    binding: Option<std::io::Error>,
}
impl ReviewedRemovalFileReport {
    #[cfg(any(unix, windows))]
    pub(super) fn from_actual(attempt: ReviewedFileRemoval) -> Self {
        Self {
            relative: attempt.relative,
            class: attempt.class,
            native: attempt.native.into_receipt(),
            #[cfg(unix)]
            append: attempt.append,
            #[cfg(unix)]
            marker: attempt.marker,
            binding: attempt.binding,
        }
    }
    pub fn relative(&self) -> &str {
        &self.relative
    }
    pub fn class(&self) -> RemovalClass {
        self.class
    }
    pub fn native(&self) -> &RemovalReceipt {
        &self.native
    }
    #[cfg(unix)]
    pub fn append(&self) -> Option<&solstone_core_journal_io::append::BoundAppendReceipt> {
        self.append.as_ref()
    }
    #[cfg(unix)]
    pub fn marker(
        &self,
    ) -> Option<
        &Result<
            solstone_core_journal_io::health_marker::BoundStreamMarkerPublication,
            solstone_core_journal_io::health_marker::BoundStreamMarkerError,
        >,
    > {
        self.marker.as_ref()
    }
    pub fn binding_error(&self) -> Option<&std::io::Error> {
        self.binding.as_ref()
    }
}

/// Resource-free facts of the actual completed selected removal.
#[must_use]
pub struct ReviewedArchiveReport {
    outcome: Outcome,
    selected: Vec<Mark>,
    attempts: Vec<ReviewedRemovalFileReport>,
    mutation: Option<
        Result<
            crate::mark_mutation::BoundMutationOutcome,
            crate::mark_mutation::BoundMutationRefusal,
        >,
    >,
    reads: Vec<ReviewedArchiveReadReport>,
}
impl ReviewedArchiveReport {
    pub fn outcome(&self) -> &Outcome {
        &self.outcome
    }
    pub fn selected_marks(&self) -> &[Mark] {
        &self.selected
    }
    pub fn attempts(&self) -> &[ReviewedRemovalFileReport] {
        &self.attempts
    }
    pub fn mutation(
        &self,
    ) -> Option<
        &Result<
            crate::mark_mutation::BoundMutationOutcome,
            crate::mark_mutation::BoundMutationRefusal,
        >,
    > {
        self.mutation.as_ref()
    }
    pub fn archive_reads(&self) -> &[ReviewedArchiveReadReport] {
        &self.reads
    }
}

/// The actual lower settled read still needs its outside runtime finish owner.
#[must_use]
pub struct SettledReviewedArchiveRemoval<'clock> {
    read: SettledDirectRead<'clock>,
    report: ReviewedArchiveReport,
}
impl<'clock> SettledReviewedArchiveRemoval<'clock> {
    pub fn report(&self) -> &ReviewedArchiveReport {
        &self.report
    }
    pub fn into_parts(self) -> (SettledDirectRead<'clock>, ReviewedArchiveReport) {
        (self.read, self.report)
    }
}

#[must_use]
pub enum ReviewedArchiveFinish<'clock> {
    Settled(Box<SettledReviewedArchiveRemoval<'clock>>),
    Held(Box<OwnedReviewedArchiveRemoval>),
}

fn close_review<'clock>(
    owner: ArchiveReviewOwner<'clock>,
) -> Result<ClosedReview, ArchiveReviewOwner<'clock>> {
    match owner {
        ArchiveReviewOwner::Pending { _read: read } => read
            .into_owned_hold()
            .map(|read| ClosedReview::Pending(Box::new(read)))
            .map_err(|read| ArchiveReviewOwner::Pending { _read: read }),
        ArchiveReviewOwner::Approval(Ok(batch)) => batch
            .into_owned_hold()
            .map(|owner| ClosedReview::Approval(Box::new(owner)))
            .map_err(|batch| ArchiveReviewOwner::Approval(Ok(batch))),
        ArchiveReviewOwner::Approval(Err(failure)) => failure
            .into_owned_hold()
            .map(|owner| ClosedReview::Approval(Box::new(owner)))
            .map_err(|failure| ArchiveReviewOwner::Approval(Err(failure))),
        ArchiveReviewOwner::Finishing(owner) => owner
            .into_owned_hold()
            .map(|owner| ClosedReview::Approval(Box::new(owner)))
            .map_err(|owner| ArchiveReviewOwner::Finishing(Box::new(owner))),
    }
}

impl<'clock> ReviewedArchiveRemoval<'clock> {
    pub fn into_owned_hold(self) -> Result<OwnedReviewedArchiveRemoval, Box<Self>> {
        let Self {
            checked,
            execution,
            approval,
        } = self;
        match close_review(approval) {
            Ok(approval) => Ok(OwnedReviewedArchiveRemoval {
                checked,
                execution,
                approval,
                finish_refusal: None,
            }),
            Err(approval) => Err(Box::new(Self {
                checked,
                execution,
                approval,
            })),
        }
    }
    fn hold(
        self,
        reason: ReviewedArchiveFinishRefusal,
    ) -> Result<ReviewedArchiveFinish<'clock>, Box<Self>> {
        self.into_owned_hold().map(|mut held| {
            held.finish_refusal = Some(reason);
            ReviewedArchiveFinish::Held(Box::new(held))
        })
    }
    pub fn finish(self) -> Result<ReviewedArchiveFinish<'clock>, Box<Self>> {
        let ArchiveReviewOwner::Approval(Ok(batch)) = &self.approval else {
            return self.hold(ReviewedArchiveFinishRefusal::IncompleteSelection);
        };
        if !batch.has_owned_clock() {
            return Err(Box::new(self));
        }
        if !complete_selection(&self.checked, &self.execution) {
            return self.hold(ReviewedArchiveFinishRefusal::IncompleteSelection);
        }
        if !batch.ready_for_finish() {
            return self.hold(ReviewedArchiveFinishRefusal::ActorResources);
        }
        self.finish_complete()
    }
}

#[cfg(any(unix, windows))]
fn complete_selection(
    checked: &crate::review_context::CheckedReviewContext,
    execution: &ReviewedExecution,
) -> bool {
    execution.carrier.is_none() && complete_selected_files(checked, execution)
}

#[cfg(any(unix, windows))]
pub(super) fn complete_selected_files(
    checked: &crate::review_context::CheckedReviewContext,
    execution: &ReviewedExecution,
) -> bool {
    use crate::mark_mutation::{BoundMutationOutcome, MutationStatus};
    use solstone_core_journal_io::media::{Confirmation, OriginalName, RemovalEffect};
    use std::collections::BTreeSet;
    if checked.marks().is_empty()
        || execution.refusal.is_some()
        || !execution.stage_failures.is_empty()
        || execution.carrier_failure.is_some()
        || execution.outcome.halted.is_some()
        || execution.outcome.targets.len() != checked.marks().len()
        || execution
            .outcome
            .targets
            .iter()
            .any(|row| !row.not_removed.is_empty() || row.post_commit_failure.is_some())
    {
        return false;
    }
    let result = match &execution.mutation {
        Some(Ok(
            BoundMutationOutcome::Published(result) | BoundMutationOutcome::Unchanged(result),
        )) => result,
        _ => return false,
    };
    if !matches!(
        result.status,
        MutationStatus::Applied | MutationStatus::AlreadyApplied
    ) || checked
        .marks()
        .iter()
        .any(|mark| result.register.marks.contains_key(&mark.id))
    {
        return false;
    }
    let expected = checked
        .marks()
        .iter()
        .flat_map(|mark| {
            mark.proposal
                .names
                .iter()
                .map(move |name| reviewed_relative(mark, name))
        })
        .collect::<BTreeSet<_>>();
    let actual = execution
        .attempts
        .iter()
        .map(|attempt| attempt.relative.clone())
        .collect::<BTreeSet<_>>();
    !expected.is_empty()
        && actual.len() == execution.attempts.len()
        && actual == expected
        && execution.attempts.iter().all(|attempt| {
            let receipt = attempt.native.receipt();
            attempt.settled
                && receipt.effect == RemovalEffect::Deleted
                && receipt.original_name == OriginalName::Absent
                && receipt.claim_confirmation == Confirmation::Confirmed
                && receipt.cleanup == Cleanup::Complete
        })
}

#[cfg(not(any(unix, windows)))]
fn complete_selection(
    _: &crate::review_context::CheckedReviewContext,
    _: &ReviewedExecution,
) -> bool {
    false
}

impl<'clock> ReviewedArchiveRemoval<'clock> {
    #[cfg(not(any(unix, windows)))]
    fn finish_complete(self) -> Result<ReviewedArchiveFinish<'clock>, Box<Self>> {
        self.hold(ReviewedArchiveFinishRefusal::IncompleteSelection)
    }
    #[cfg(any(unix, windows))]
    fn finish_complete(self) -> Result<ReviewedArchiveFinish<'clock>, Box<Self>> {
        let Self {
            checked,
            mut execution,
            approval,
        } = self;
        let ArchiveReviewOwner::Approval(Ok(batch)) = approval else {
            return Self {
                checked,
                execution,
                approval,
            }
            .hold(ReviewedArchiveFinishRefusal::IncompleteSelection);
        };
        let owner = batch.settle();
        let refusal = owner
            .readback_error()
            .copied()
            .map(ReviewedArchiveFinishRefusal::Readback)
            .or_else(|| finish_native(&mut execution, owner.carrier()).err());
        if let Some(reason) = refusal {
            return Self {
                checked,
                execution,
                approval: ArchiveReviewOwner::Finishing(Box::new(owner)),
            }
            .hold(reason);
        }
        let (read, reads) = match owner.release() {
            Ok(parts) => parts,
            Err(owner) => {
                return Self {
                    checked,
                    execution,
                    approval: ArchiveReviewOwner::Finishing(Box::new(owner)),
                }
                .hold(ReviewedArchiveFinishRefusal::ActorResources);
            }
        };
        let report = release_report(checked, execution, reads);
        Ok(ReviewedArchiveFinish::Settled(Box::new(
            SettledReviewedArchiveRemoval { read, report },
        )))
    }
}

#[cfg(any(unix, windows))]
pub(super) fn finish_native(
    execution: &mut ReviewedExecution,
    carrier: &solstone_core_journal_io::media::RootActivityCarrier,
) -> Result<(), ReviewedArchiveFinishRefusal> {
    let validate = (|| {
        carrier.revalidate()?;
        carrier
            .activity()
            .revalidate()
            .map_err(std::io::Error::other)?;
        carrier
            .journal_root()
            .revalidate_canonical_binding()
            .map_err(std::io::Error::other)
    })();
    validate.map_err(ReviewedArchiveFinishRefusal::Changed)?;
    for attempt in &mut execution.attempts {
        let cleanup = attempt.native.cleanup();
        if cleanup != Cleanup::Complete {
            return Err(ReviewedArchiveFinishRefusal::NativeCleanup(cleanup));
        }
    }
    let obligations = observe_finish_census(
        || {
            solstone_core_journal_io::activity_obligations::observe_with_carrier(
                carrier.journal_root(),
                carrier.observation(),
            )
        },
        || std::thread::sleep(std::time::Duration::from_millis(100)),
    )
    .map_err(ReviewedArchiveFinishRefusal::Census)?;
    if obligations.unresolved_entries().next().is_some() {
        return Err(ReviewedArchiveFinishRefusal::Obligations(
            obligations.entries().to_vec(),
        ));
    }
    Ok(())
}

#[cfg(any(unix, windows))]
const FINISH_CENSUS_ATTEMPTS: usize = 3;

/// A live journal keeps writing while the census lists each directory twice,
/// and Windows widens that window, so one pass can refuse a journal with
/// nothing owed. Settlement still needs one fresh complete census: a retry
/// never reuses a partial walk. A non-directory entry or an exhausted budget
/// does not retry; an IO refusal does, even though some are permanent, since
/// a read-only census costs nothing to repeat.
#[cfg(any(unix, windows))]
fn observe_finish_census<T>(
    mut observe: impl FnMut() -> Result<T, RecoveryObservationError>,
    mut pause: impl FnMut(),
) -> Result<T, RecoveryObservationError> {
    let mut attempt = 1;
    loop {
        match observe() {
            Ok(complete) => return Ok(complete),
            Err(error)
                if attempt < FINISH_CENSUS_ATTEMPTS
                    && matches!(
                        error.failure,
                        Failure::Io | Failure::Changed | Failure::DirectoryAdded
                    ) =>
            {
                log::warn!("finish census attempt {attempt} refused, retrying: {error}");
                attempt = attempt.saturating_add(1);
                pause();
            }
            Err(error) => return Err(error),
        }
    }
}

#[cfg(any(unix, windows))]
fn release_report(
    checked: crate::review_context::CheckedReviewContext,
    execution: ReviewedExecution,
    reads: Vec<ReviewedArchiveReadReport>,
) -> ReviewedArchiveReport {
    let selected = checked.marks().to_vec();
    let attempts = execution
        .attempts
        .into_iter()
        .map(ReviewedRemovalFileReport::from_actual)
        .collect();
    ReviewedArchiveReport {
        outcome: execution.outcome,
        selected,
        attempts,
        mutation: execution.mutation,
        reads,
    }
}

/// The typed worker's original read and all actual effects, before settlement.
#[must_use]
pub struct OwnedReviewedArchiveRun {
    checked: crate::review_context::CheckedReviewContext,
    execution: ReviewedExecution,
    approval: OwnedArchiveReviewOwner,
}

enum OwnedArchiveReviewOwner {
    Pending(Box<solstone_core_backup::archive_readback::OwnedReadOperation>),
    Approval(
        Result<
            crate::archive_approval::OwnedArchiveApprovalBatch,
            crate::archive_approval::OwnedArchiveApprovalFailure,
        >,
    ),
}

/// Execute the same checked removal using a read whose every refusal can close.
pub fn remove_reviewed_with_owned_archive(
    checked: crate::review_context::CheckedReviewContext,
    read: solstone_core_backup::archive_readback::OwnedReadOperation,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> OwnedReviewedArchiveRun {
    #[cfg(any(unix, windows))]
    let selected = preflight_archive_review(&checked, read.as_read());
    #[cfg(not(any(unix, windows)))]
    let selected: Result<Vec<Mark>, ReviewedRemovalRefusal> =
        Err(ReviewedRemovalRefusal::Unsupported);
    let mut execution = ReviewedExecution::new();
    let selected = match selected {
        Ok(selected) => selected,
        Err(reason) => {
            finish_archive_execution(&mut execution, Err(reason));
            return OwnedReviewedArchiveRun {
                checked,
                execution,
                approval: OwnedArchiveReviewOwner::Pending(Box::new(read)),
            };
        }
    };
    let approval = crate::archive_approval::create_owned_archive_approval_batch(read, &selected);
    #[cfg(any(unix, windows))]
    let result = match &approval {
        Ok(batch) => execute_reviewed_mode(
            &checked,
            today,
            now,
            &mut execution,
            Some(batch.view()),
            |_, _| {},
        ),
        Err(_) => Err(ReviewedRemovalRefusal::ArchiveApproval),
    };
    #[cfg(not(any(unix, windows)))]
    let result = {
        let _parameters = (today, now);
        Err(ReviewedRemovalRefusal::Unsupported)
    };
    finish_archive_execution(&mut execution, result);
    OwnedReviewedArchiveRun {
        checked,
        execution,
        approval: OwnedArchiveReviewOwner::Approval(approval),
    }
}

impl OwnedReviewedArchiveRun {
    pub fn outcome(&self) -> &Outcome {
        &self.execution.outcome
    }
    pub fn refusal(&self) -> Option<&ReviewedRemovalRefusal> {
        self.execution.refusal.as_ref()
    }
    pub fn attempts(&self) -> &[ReviewedFileRemoval] {
        &self.execution.attempts
    }
    pub fn stage_failures(
        &self,
    ) -> &[solstone_core_journal_io::media::OwnedMediaFailure<
        Option<solstone_core_journal_io::media::MediaStage>,
    >] {
        &self.execution.stage_failures
    }
    pub fn mutation(
        &self,
    ) -> Option<
        &Result<
            crate::mark_mutation::BoundMutationOutcome,
            crate::mark_mutation::BoundMutationRefusal,
        >,
    > {
        self.execution.mutation.as_ref()
    }
    pub fn journal_root(&self) -> &solstone_core_journal_io::JournalRoot {
        self.checked.root()
    }
    pub fn selected_marks(&self) -> &[Mark] {
        self.checked.marks()
    }
    pub fn archive_journal_root(&self) -> &std::sync::Arc<solstone_core_journal_io::JournalRoot> {
        match &self.approval {
            OwnedArchiveReviewOwner::Pending(read) => read.journal_root(),
            OwnedArchiveReviewOwner::Approval(Ok(batch)) => batch.view().journal_root(),
            OwnedArchiveReviewOwner::Approval(Err(failure)) => failure.journal_root(),
        }
    }
    pub fn archive_activity(
        &self,
    ) -> &std::sync::Arc<solstone_core_journal_io::activity::ActivityLease> {
        match &self.approval {
            OwnedArchiveReviewOwner::Pending(read) => read.activity(),
            OwnedArchiveReviewOwner::Approval(Ok(batch)) => batch.view().activity(),
            OwnedArchiveReviewOwner::Approval(Err(failure)) => failure.activity(),
        }
    }
    pub fn archive_expected_marks(&self) -> Option<&[Mark]> {
        match &self.approval {
            OwnedArchiveReviewOwner::Pending(_) => None,
            OwnedArchiveReviewOwner::Approval(Ok(batch)) => Some(batch.view().expected_marks()),
            OwnedArchiveReviewOwner::Approval(Err(failure)) => Some(failure.expected_marks()),
        }
    }
    pub fn archive_reads(&self) -> impl Iterator<Item = ArchiveReadState<'_>> {
        match &self.approval {
            OwnedArchiveReviewOwner::Pending(_) => {
                crate::archive_approval::ArchiveReadStates::empty()
            }
            OwnedArchiveReviewOwner::Approval(Ok(batch)) => batch.read_states(),
            OwnedArchiveReviewOwner::Approval(Err(failure)) => failure.read_states(),
        }
    }
    fn approval_error(&self) -> Option<&ArchiveApprovalError> {
        let OwnedArchiveReviewOwner::Approval(Err(failure)) = &self.approval else {
            return None;
        };
        Some(failure.reason())
    }
    pub fn approval_readback_error(&self) -> Option<&ReadbackError> {
        if let Some(ReviewedRemovalRefusal::Readback(reason)) = self.refusal() {
            return Some(reason);
        }
        match self.approval_error()? {
            ArchiveApprovalError::Readback(reason) => Some(reason),
            _ => None,
        }
    }
    pub fn approval_cleanup(&self) -> Option<Cleanup> {
        match self.approval_error()? {
            ArchiveApprovalError::Cleanup(cleanup) => Some(*cleanup),
            _ => None,
        }
    }
    pub fn approval_native_error(&self) -> Option<&std::io::Error> {
        match self.approval_error()? {
            ArchiveApprovalError::Native(error) => Some(error),
            _ => None,
        }
    }
    pub fn into_owned_hold(self) -> OwnedReviewedArchiveRemoval {
        let approval = match self.approval {
            OwnedArchiveReviewOwner::Pending(read) => {
                ClosedReview::Pending(Box::new(read.into_owned_hold()))
            }
            OwnedArchiveReviewOwner::Approval(Ok(batch)) => {
                ClosedReview::Approval(Box::new(batch.into_owned_hold()))
            }
            OwnedArchiveReviewOwner::Approval(Err(failure)) => {
                ClosedReview::Approval(Box::new(failure.into_owned_hold()))
            }
        };
        OwnedReviewedArchiveRemoval {
            checked: self.checked,
            execution: self.execution,
            approval,
            finish_refusal: None,
        }
    }
    fn hold(self, reason: ReviewedArchiveFinishRefusal) -> OwnedReviewedArchiveFinish {
        let mut closed = self.into_owned_hold();
        closed.finish_refusal = Some(reason);
        OwnedReviewedArchiveFinish::Held(Box::new(closed))
    }
    pub fn finish(self) -> OwnedReviewedArchiveFinish {
        let OwnedArchiveReviewOwner::Approval(Ok(batch)) = &self.approval else {
            return self.hold(ReviewedArchiveFinishRefusal::IncompleteSelection);
        };
        if !complete_selection(&self.checked, &self.execution) {
            return self.hold(ReviewedArchiveFinishRefusal::IncompleteSelection);
        }
        if !batch.ready_for_finish() {
            return self.hold(ReviewedArchiveFinishRefusal::ActorResources);
        }
        self.finish_complete()
    }
    #[cfg(not(any(unix, windows)))]
    fn finish_complete(self) -> OwnedReviewedArchiveFinish {
        self.hold(ReviewedArchiveFinishRefusal::IncompleteSelection)
    }
    #[cfg(any(unix, windows))]
    fn finish_complete(self) -> OwnedReviewedArchiveFinish {
        let Self {
            checked,
            mut execution,
            approval,
        } = self;
        let OwnedArchiveReviewOwner::Approval(Ok(batch)) = approval else {
            return Self {
                checked,
                execution,
                approval,
            }
            .hold(ReviewedArchiveFinishRefusal::IncompleteSelection);
        };
        let owner = batch.settle();
        let refusal = owner
            .readback_error()
            .map(ReviewedArchiveFinishRefusal::Readback)
            .or_else(|| finish_native(&mut execution, owner.carrier()).err());
        if let Some(reason) = refusal {
            return OwnedReviewedArchiveFinish::Held(Box::new(OwnedReviewedArchiveRemoval {
                checked,
                execution,
                approval: ClosedReview::Approval(Box::new(owner.into_owned_hold())),
                finish_refusal: Some(reason),
            }));
        }
        let (read, reads) = match owner.release() {
            Ok(parts) => parts,
            Err(owner) => {
                return OwnedReviewedArchiveFinish::Held(Box::new(OwnedReviewedArchiveRemoval {
                    checked,
                    execution,
                    approval: ClosedReview::Approval(Box::new(owner.into_owned_hold())),
                    finish_refusal: Some(ReviewedArchiveFinishRefusal::ActorResources),
                }));
            }
        };
        let report = release_report(checked, execution, reads);
        OwnedReviewedArchiveFinish::Settled(Box::new(OwnedSettledReviewedArchiveRemoval {
            read,
            report,
        }))
    }
}

#[must_use]
pub enum OwnedReviewedArchiveFinish {
    Settled(Box<OwnedSettledReviewedArchiveRemoval>),
    Held(Box<OwnedReviewedArchiveRemoval>),
}
#[must_use]
pub struct OwnedSettledReviewedArchiveRemoval {
    read: solstone_core_backup::archive_readback::OwnedSettledDirectRead,
    report: ReviewedArchiveReport,
}
impl OwnedSettledReviewedArchiveRemoval {
    pub fn report(&self) -> &ReviewedArchiveReport {
        &self.report
    }
    pub fn into_parts(
        self,
    ) -> (
        solstone_core_backup::archive_readback::OwnedSettledDirectRead,
        ReviewedArchiveReport,
    ) {
        (self.read, self.report)
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn closed_reviewed_archive_custody_and_report_are_send_static() {
        fn check<T: Send + 'static>() {}
        check::<super::OwnedReviewedArchiveRemoval>();
        check::<super::ReviewedArchiveReport>();
        check::<super::OwnedSettledReviewedArchiveRemoval>();
        check::<super::OwnedReviewedArchiveFinish>();
    }

    #[cfg(any(unix, windows))]
    #[allow(
        clippy::unwrap_used,
        clippy::panic,
        reason = "test code; the crate-level denials exist to constrain the verbs"
    )]
    mod finish_census {
        use super::super::{Failure, RecoveryObservationError, observe_finish_census};
        use std::path::PathBuf;

        fn refused(failure: Failure) -> RecoveryObservationError {
            RecoveryObservationError {
                failure,
                relative: PathBuf::from("chronicle/20261001"),
                source: None,
            }
        }

        #[test]
        fn a_listing_that_moves_under_a_live_journal_settles_on_a_fresh_complete_census() {
            for failure in [Failure::Io, Failure::Changed, Failure::DirectoryAdded] {
                let (mut calls, mut pauses) = (0, 0);
                let census = observe_finish_census(
                    || {
                        calls += 1;
                        if calls < 3 {
                            Err(refused(failure))
                        } else {
                            Ok("complete")
                        }
                    },
                    || pauses += 1,
                );
                assert_eq!(census.unwrap(), "complete");
                assert_eq!((calls, pauses), (3, 2));
            }
        }

        #[test]
        fn a_census_that_never_settles_refuses_with_its_last_failure() {
            let mut calls = 0;
            let refusal = observe_finish_census::<()>(
                || {
                    calls += 1;
                    Err(RecoveryObservationError {
                        failure: if calls < 3 {
                            Failure::Changed
                        } else {
                            Failure::Io
                        },
                        relative: PathBuf::from(format!("pass-{calls}")),
                        source: None,
                    })
                },
                || {},
            )
            .unwrap_err();
            assert_eq!(calls, 3);
            assert_eq!(refusal.failure, Failure::Io);
            assert_eq!(refusal.relative, PathBuf::from("pass-3"));
        }

        #[test]
        fn a_refusal_no_later_pass_can_change_never_retries() {
            for failure in [
                Failure::NotDirectory,
                Failure::Budget(
                    solstone_core_journal_io::activity_obligations::RecoveryBudget::Obligations,
                ),
            ] {
                let mut calls = 0;
                let refusal = observe_finish_census::<()>(
                    || {
                        calls += 1;
                        Err(refused(failure))
                    },
                    || panic!("a permanent refusal cannot pause or retry"),
                )
                .unwrap_err();
                assert_eq!((calls, refusal.failure), (1, failure));
            }
        }
    }
}
