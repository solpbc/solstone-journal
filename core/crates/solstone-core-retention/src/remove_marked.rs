// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Execute approved raw-release marks after proving them again from disk.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;
use std::sync::Arc;

use chrono::{DateTime, NaiveDate, SecondsFormat, Utc};
use solstone_core_journal_io::activity::ActivityLease;
use solstone_core_journal_io::{LockOptions, hold_lock};

use crate::class::partition_empty_audio;
use crate::content::{ClosedHandlerSet, JournalMedia};
use crate::door::{
    admit_legacy_raw_activity, release_raw_with_activity, require_legacy_raw_target,
};
use crate::eligibility::{Blocker, FoundContent, ProvenRaw};
use crate::grouped::{DecisionPurpose, SideDecision};
use crate::mark_mutation::{PlannedMarkMutation, conditional_mutate};
use crate::marks::{
    Failure, Mark, MarkId, PreflightMarks, PreflightRefusal, RemovalClass, load as load_marks,
};
use crate::original_deletion::RawReleaseClass;
use crate::receipt::{NotRemoved, Outcome, RunHalt, TargetOutcome};
use crate::scan::scan_segment;
use crate::{Eligibility, Policy};

const TOO_YOUNG: &str = "this one isn't old enough to delete yet.";
const KEPT_FOREVER: &str = "this one is kept indefinitely.";
const ANCHOR_MISSING: &str = "there's no date on this one, so it can't be deleted.";
const NOT_ON_REMOVAL_LIST: &str =
    "this file was proven releasable but is not on the removal list, so it is left in place";
const NO_LONGER_PRESENT: &str = "this file was on the removal list but is no longer present";
const NO_LONGER_ON_REMOVAL_LIST: &str =
    "this entry is no longer on the removal list, so nothing was removed";

struct RemovalContext<'a> {
    activity: &'a Arc<ActivityLease>,
    halted: Option<String>,
    policy: &'a Policy,
    today: NaiveDate,
    now: DateTime<Utc>,
    register_errors: &'a mut Vec<String>,
}

/// Execute marks already proven valid by preflight.
///
/// A row describes a target the run reached; `halted` describes the target it
/// could not start and every requested target after it.
pub fn remove_marked(
    journal: &Path,
    marks: &PreflightMarks,
    policy: &Policy,
    today: NaiveDate,
    now: DateTime<Utc>,
    register_errors: &mut Vec<String>,
) -> Outcome {
    if marks.as_slice().is_empty() {
        return Outcome {
            targets: Vec::new(),
            halted: None,
        };
    }
    let activity = match admit_legacy_raw_activity(journal) {
        Ok(activity) => activity,
        Err(reason) => return Outcome::halted_before_start(reason),
    };
    let journal = activity.journal_root().canonical_path();
    let mut context = RemovalContext {
        activity: &activity,
        halted: None,
        policy,
        today,
        now,
        register_errors,
    };
    let mut outcome = Outcome {
        targets: Vec::new(),
        halted: None,
    };
    for (index, (id, mark)) in marks.as_slice().iter().enumerate() {
        match remove_one(journal, id, mark, &mut context) {
            Ok(row) => {
                outcome.targets.push(row);
                if let Some(reason) = context.halted.take() {
                    outcome.halted = Some(RunHalt { reason });
                    break;
                }
            }
            Err(()) => {
                let remaining = marks.as_slice().len().saturating_sub(index);
                let reason = format!(
                    "the originals for {} couldn't be started on because something else is using them. the rest of the removal list wasn't attempted ({remaining} remaining).",
                    id.as_str()
                );
                if outcome.targets.is_empty() {
                    return Outcome::halted_before_start(reason);
                }
                outcome.halted = Some(RunHalt { reason });
                break;
            }
        }
    }
    outcome
}

const UNBOUND_OFFLOAD: &str =
    "this offload proposal has no record of its backup copy, so its originals were kept";

fn remove_one(
    journal: &Path,
    id: &MarkId,
    mark: &Mark,
    context: &mut RemovalContext<'_>,
) -> Result<TargetOutcome, ()> {
    let target = &mark.target;
    if mark.offload.is_some() {
        return Ok(refused(
            mark,
            &PreflightRefusal::ArchiveBound { id: id.clone() }.to_string(),
        ));
    }
    // An offload proposal without its backup proof never falls through to the
    // retention policy: the policy verdict is not evidence of a backup copy.
    if mark.class == RemovalClass::OffloadRawRelease {
        return Ok(refused(mark, UNBOUND_OFFLOAD));
    }
    let live = crate::layout::segment_rel(&target.day, &target.stream, &target.dir);
    if let Err(reason) = require_legacy_raw_target(context.activity, target) {
        context.halted = Some(reason.clone());
        return Ok(refused(mark, &reason));
    }
    let _segment_lock = hold_lock(journal.join(&live), LockOptions::default()).map_err(|_| ())?;
    let (row, complete, staged) = {
        let register = match load_marks(journal) {
            Ok(register) => register,
            Err(error) => return Ok(refused(mark, &error.to_string())),
        };
        if register.version != 1 {
            return Ok(refused(
                mark,
                &crate::marks::StoreError::WriterHeld.to_string(),
            ));
        }
        if register.marks.get(id) != Some(mark) {
            return Ok(refused(mark, NO_LONGER_ON_REMOVAL_LIST));
        }
        let raw_class = match mark.class {
            RemovalClass::PolicyRawRelease => RawReleaseClass::Policy,
            RemovalClass::OffloadRawRelease => RawReleaseClass::Offload,
            RemovalClass::OwnerRawRelease | RemovalClass::OwnerSegmentRemoval => {
                let reason =
                    "this mark is not an original-media deletion that can be approved from the removal list"
                        .to_owned();
                let not_removed = mark
                    .proposal
                    .names
                    .iter()
                    .map(|name| NotRemoved {
                        entry: crate::layout::content_rel(
                            &target.day,
                            &target.stream,
                            &target.dir,
                            name,
                        ),
                        reason: reason.clone(),
                        staged: None,
                    })
                    .collect();
                return Ok(TargetOutcome {
                    target: target.clone(),
                    removed: Vec::new(),
                    not_removed,
                    post_commit_failure: None,
                });
            }
        };
        let found = scan_segment(&journal.join(&live), &ClosedHandlerSet, &JournalMedia);
        let found_names = found
            .iter()
            .map(|item| item.name.as_str().to_owned())
            .collect::<BTreeSet<_>>();
        let (empty, ordinary) = partition_empty_audio(found, &ClosedHandlerSet);
        let empty_side = decide_side(&empty, target, context);
        let ordinary_side = decide_side(&ordinary, target, context);
        let mut side_refusal = BTreeMap::new();
        record_side_refusal(&empty, &empty_side, &mut side_refusal);
        record_side_refusal(&ordinary, &ordinary_side, &mut side_refusal);
        let ready = ready_union(empty_side, ordinary_side);
        let proved_names = ready
            .iter()
            .map(|item| item.name().to_owned())
            .collect::<BTreeSet<_>>();
        let desired = mark
            .proposal
            .names
            .iter()
            .map(String::as_str)
            .collect::<BTreeSet<_>>();
        let not_approved = ready
            .iter()
            .filter(|item| !desired.contains(item.name()))
            .map(|item| item.name().to_owned())
            .collect::<Vec<_>>();
        let ready = ready
            .into_iter()
            .filter(|item| desired.contains(item.name()))
            .collect::<Vec<_>>();
        let (partial, _) =
            release_raw_with_activity(context.activity, None, &ready, raw_class, context.now);
        let mut row = partial.targets.into_iter().next().unwrap_or(TargetOutcome {
            target: target.clone(),
            removed: Vec::new(),
            not_removed: Vec::new(),
            post_commit_failure: None,
        });
        for name in not_approved {
            row.not_removed.push(NotRemoved {
                entry: crate::layout::content_rel(&target.day, &target.stream, &target.dir, &name),
                reason: NOT_ON_REMOVAL_LIST.to_owned(),
                staged: None,
            });
        }
        for name in &mark.proposal.names {
            if !found_names.contains(name.as_str()) {
                row.not_removed.push(NotRemoved {
                    entry: crate::layout::content_rel(
                        &target.day,
                        &target.stream,
                        &target.dir,
                        name,
                    ),
                    reason: NO_LONGER_PRESENT.to_owned(),
                    staged: None,
                });
            }
        }
        for name in &mark.proposal.names {
            if found_names.contains(name.as_str())
                && !proved_names.contains(name)
                && let Some(reason) = side_refusal.get(name)
            {
                row.not_removed.push(NotRemoved {
                    entry: crate::layout::content_rel(
                        &target.day,
                        &target.stream,
                        &target.dir,
                        name,
                    ),
                    reason: reason.clone(),
                    staged: None,
                });
            }
        }
        let removed = row
            .removed
            .iter()
            .map(|path| path.as_str())
            .collect::<BTreeSet<_>>();
        let accounted = mark.proposal.names.iter().all(|name| {
            let rel = crate::layout::content_rel(&target.day, &target.stream, &target.dir, name);
            removed.contains(rel.as_str())
                || row
                    .not_removed
                    .iter()
                    .any(|item| item.reason == NO_LONGER_PRESENT && item.entry == rel)
        });
        let complete = accounted && !row.not_removed.iter().any(|item| item.staged.is_some());
        let staged = row.not_removed.iter().find_map(|item| {
            item.staged
                .as_ref()
                .map(|path| (path.to_owned(), item.reason.to_owned()))
        });
        (row, complete, staged)
    };
    // Segment lock, then register lock, always: marks owns the second lock internally.
    #[cfg(all(test, unix, feature = "full-tests"))]
    crate::door::cooperation_tests::checkpoint(
        crate::door::cooperation_tests::Phase::BeforeRawCas,
        target,
    );
    if let Err(reason) = require_legacy_raw_target(context.activity, target) {
        context.register_errors.push(reason.clone());
        context.halted = Some(reason);
        return Ok(row);
    }
    let plan = if complete {
        Some(PlannedMarkMutation::resolve(mark))
    } else {
        staged.map(|(staged, reason)| {
            PlannedMarkMutation::failure(
                mark,
                Failure {
                    at: context.now.to_rfc3339_opts(SecondsFormat::Secs, true),
                    reason,
                    staged: Some(staged),
                },
            )
        })
    };
    if let Some(plan) = plan
        && let Err(error) = plan.and_then(|plan| conditional_mutate(journal, &plan))
    {
        context.register_errors.push(error.to_string());
    }
    Ok(row)
}

enum SideOutcome {
    None,
    Policy(Eligibility),
    Held(Vec<Blocker>),
    Proved(Vec<ProvenRaw>),
}

fn decide_side(
    side: &[FoundContent],
    target: &crate::Target,
    context: &RemovalContext<'_>,
) -> SideOutcome {
    match crate::grouped::decide_side(
        side,
        &ClosedHandlerSet,
        &JournalMedia,
        context.policy,
        target,
        context.today,
        context.now,
        DecisionPurpose::PolicyRelease,
    ) {
        SideDecision::NoMedia => SideOutcome::None,
        SideDecision::Kept(verdict) => SideOutcome::Policy(verdict),
        SideDecision::Unproven(blockers) => SideOutcome::Held(blockers),
        SideDecision::Eligible { proven, .. } => SideOutcome::Proved(proven),
    }
}

fn ready_union(empty: SideOutcome, ordinary: SideOutcome) -> Vec<ProvenRaw> {
    let mut ready = match empty {
        SideOutcome::Proved(proven) => proven,
        _ => Vec::new(),
    };
    if let SideOutcome::Proved(proven) = ordinary {
        ready.extend(proven);
    }
    ready
}

fn record_side_refusal(
    side: &[FoundContent],
    outcome: &SideOutcome,
    side_refusal: &mut BTreeMap<String, String>,
) {
    let reason = match outcome {
        SideOutcome::Policy(Eligibility::TooYoung { .. }) => TOO_YOUNG.to_owned(),
        SideOutcome::Policy(Eligibility::KeptForever) => KEPT_FOREVER.to_owned(),
        SideOutcome::Policy(Eligibility::AnchorMissing { .. }) => ANCHOR_MISSING.to_owned(),
        SideOutcome::Policy(Eligibility::Eligible { .. })
        | SideOutcome::None
        | SideOutcome::Proved(_) => {
            return;
        }
        SideOutcome::Held(blockers) => format!(
            "the current processing proof no longer permits this release: {}",
            blockers
                .iter()
                .map(|blocker| blocker.name())
                .collect::<Vec<_>>()
                .join(", ")
        ),
    };
    for item in side {
        side_refusal.insert(item.name.as_str().to_owned(), reason.clone());
    }
}

fn refused(mark: &Mark, reason: &str) -> TargetOutcome {
    TargetOutcome {
        target: mark.target.clone(),
        removed: Vec::new(),
        not_removed: vec![NotRemoved {
            entry: crate::layout::segment_rel(
                &mark.target.day,
                &mark.target.stream,
                &mark.target.dir,
            ),
            reason: reason.to_owned(),
            staged: None,
        }],
        post_commit_failure: None,
    }
}

const REVIEW_REFUSED: &str =
    "the originals couldn't be deleted because their current state couldn't be confirmed.";
#[cfg(any(unix, windows))]
const REVIEW_UNCERTAIN: &str = "deletion couldn't be completed or confirmed. recovery is needed.";
const REVIEW_POST_COMMIT: &str =
    "this file was deleted, but the change or its records couldn't be fully saved.";
const REVIEW_HALTED: &str =
    "the remaining originals were left in place because the current removal couldn't be completed.";

/// Diagnostic facts; no variant supplies release or cleanup authority.
#[derive(Debug)]
pub enum ReviewedRemovalRefusal {
    Unsupported,
    InvalidSelection,
    Io(std::io::Error),
    Mutation(crate::mark_mutation::BoundMutationRefusal),
    Activity(solstone_core_journal_io::activity::ActivityAdmissionError),
    Census(solstone_core_journal_io::activity_obligations::RecoveryObservationError),
    Obligations(solstone_core_journal_io::activity_obligations::NativeObligations),
    Configuration(solstone_core_journal_config::ConfigLoadError),
    #[cfg(any(unix, windows))]
    Directory(solstone_core_journal_io::FlatDirectoryError),
    Changed,
    CarrierPreparation,
    ArchiveApproval,
    Catalog(String),
    Readback(solstone_core_backup::archive_readback::ReadbackError),
    OffloadFloorRequired,
    InsufficientSpace {
        available: u64,
        required: u64,
    },
}

impl From<std::io::Error> for ReviewedRemovalRefusal {
    fn from(value: std::io::Error) -> Self {
        Self::Io(value)
    }
}
#[cfg(any(unix, windows))]
impl From<solstone_core_journal_io::FlatDirectoryError> for ReviewedRemovalRefusal {
    fn from(value: solstone_core_journal_io::FlatDirectoryError) -> Self {
        Self::Directory(value)
    }
}
impl From<crate::mark_mutation::BoundMutationRefusal> for ReviewedRemovalRefusal {
    fn from(value: crate::mark_mutation::BoundMutationRefusal) -> Self {
        Self::Mutation(value)
    }
}

/// Actual per-file owners and bookkeeping. Drop never repairs an uncertain claim.
pub struct ReviewedFileRemoval {
    relative: String,
    class: RemovalClass,
    native: solstone_core_journal_io::media::MediaRemoval,
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
    #[cfg(any(unix, windows))]
    settled: bool,
}

impl ReviewedFileRemoval {
    pub fn relative(&self) -> &str {
        &self.relative
    }
    pub fn class(&self) -> RemovalClass {
        self.class
    }
    pub fn native(&self) -> &solstone_core_journal_io::media::MediaRemoval {
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

struct ReviewedExecution {
    outcome: Outcome,
    refusal: Option<ReviewedRemovalRefusal>,
    carrier: Option<solstone_core_journal_io::media::RootActivityCarrier>,
    carrier_failure: Option<solstone_core_journal_io::media::ActivityCarrierPreparationFailure>,
    attempts: Vec<ReviewedFileRemoval>,
    stage_failures: Vec<
        solstone_core_journal_io::media::OwnedMediaFailure<
            Option<solstone_core_journal_io::media::MediaStage>,
        >,
    >,
    mutation: Option<
        Result<
            crate::mark_mutation::BoundMutationOutcome,
            crate::mark_mutation::BoundMutationRefusal,
        >,
    >,
}

impl ReviewedExecution {
    fn new() -> Self {
        Self {
            outcome: Outcome {
                targets: Vec::new(),
                halted: None,
            },
            refusal: None,
            carrier: None,
            carrier_failure: None,
            attempts: Vec::new(),
            stage_failures: Vec::new(),
            mutation: None,
        }
    }
}

/// Owns the original review and every local effect through the caller's settlement.
/// Its consuming finish retains uncertainty and reports actual local retirement.
#[must_use]
pub struct ReviewedRemoval {
    checked: crate::review_context::CheckedReviewContext,
    execution: ReviewedExecution,
}

impl ReviewedRemoval {
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
    pub fn carrier_failure(
        &self,
    ) -> Option<&solstone_core_journal_io::media::ActivityCarrierPreparationFailure> {
        self.execution.carrier_failure.as_ref()
    }
    pub fn carrier_preparation(
        &self,
    ) -> Option<&solstone_core_journal_io::media::ActivityCarrierPreparationReceipt> {
        self.execution.carrier.as_ref().map(|c| c.preparation())
    }
    pub fn journal_root(&self) -> &solstone_core_journal_io::JournalRoot {
        self.checked.root()
    }
    pub fn selected_marks(&self) -> &[Mark] {
        self.checked.marks()
    }
}

/// Consume rendered ordinary actions beneath their original retained root.
/// Typed writers and platform/carrier settlement activation remain held.
pub fn remove_reviewed(
    checked: crate::review_context::CheckedReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> ReviewedRemoval {
    let mut execution = ReviewedExecution::new();
    #[cfg(any(unix, windows))]
    let result = execute_reviewed(&checked, today, now, &mut execution);
    #[cfg(not(any(unix, windows)))]
    let result: Result<(), ReviewedRemovalRefusal> = {
        let _parameters = (today, now);
        Err(ReviewedRemovalRefusal::Unsupported)
    };
    if let Err(error) = result {
        execution.refusal = Some(error);
        reviewed_mark_unsaved_deletions(&mut execution.outcome);
        execution.outcome.halted = Some(RunHalt {
            reason: if execution.attempts.is_empty() {
                REVIEW_REFUSED
            } else {
                REVIEW_HALTED
            }
            .to_owned(),
        });
    }
    ReviewedRemoval { checked, execution }
}

mod archive_finish;
pub use archive_finish::{
    OwnedReviewedArchiveFinish, OwnedReviewedArchiveRemoval, OwnedReviewedArchiveRun,
    OwnedSettledReviewedArchiveRemoval, ReviewedArchiveFinish, ReviewedArchiveFinishRefusal,
    ReviewedArchiveReadReport, ReviewedArchiveReport, ReviewedRemovalFileReport,
    SettledReviewedArchiveRemoval, remove_reviewed_with_owned_archive,
};
mod interrupted;
pub use interrupted::{InterruptedRemoval, recover_interrupted_removals};
mod ordinary_finish;
pub use ordinary_finish::{
    OwnedReviewedRemoval, ReviewedRemovalFinish, ReviewedRemovalFinishRefusal,
    ReviewedRemovalReport,
};

/// Retains exact archive confirmation and all effects through caller settlement.
#[must_use]
pub struct ReviewedArchiveRemoval<'a> {
    checked: crate::review_context::CheckedReviewContext,
    approval: ArchiveReviewOwner<'a>,
    execution: ReviewedExecution,
}

enum ArchiveReviewOwner<'a> {
    Finishing(Box<crate::archive_approval::ArchiveApprovalFinishing<'a>>),
    Pending {
        _read: Box<solstone_core_backup::archive_readback::ReadOperation<'a>>,
    },
    Approval(
        Result<
            crate::archive_approval::ArchiveApprovalBatch<'a>,
            crate::archive_approval::ArchiveApprovalFailure<'a>,
        >,
    ),
}

/// Borrowed retained resources and observed facts, without cleanup authority.
pub enum ArchiveReadState<'a> {
    Stage(Option<&'a solstone_core_journal_io::media::MediaStage>),
    Target(&'a solstone_core_journal_io::media::OwnedMediaTarget),
    Prepared {
        file: &'a solstone_core_backup::archive_readback::ExactArchiveFile,
        target_identity: &'a solstone_core_journal_io::media::MediaIdentity,
        media: &'a solstone_core_journal_io::media::PreparedMedia,
        cleanup: Option<solstone_core_journal_io::media::Cleanup>,
    },
    Readback {
        file: &'a solstone_core_backup::archive_readback::ExactArchiveFile,
        target_identity: &'a solstone_core_journal_io::media::MediaIdentity,
        failure: &'a solstone_core_backup::archive_readback::ReadbackFailure,
        cleanup: solstone_core_journal_io::media::Cleanup,
    },
}

impl ReviewedArchiveRemoval<'_> {
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
    pub fn approval_readback_error(
        &self,
    ) -> Option<&solstone_core_backup::archive_readback::ReadbackError> {
        if let Some(ReviewedRemovalRefusal::Readback(error)) = self.refusal() {
            return Some(error);
        }
        if let ArchiveReviewOwner::Finishing(owner) = &self.approval {
            return owner.readback_error();
        }
        let ArchiveReviewOwner::Approval(approval) = &self.approval else {
            return None;
        };
        match approval.as_ref().err()?.reason() {
            crate::archive_approval::ArchiveApprovalError::Readback(error) => Some(error),
            _ => None,
        }
    }
    pub fn approval_cleanup(&self) -> Option<solstone_core_journal_io::media::Cleanup> {
        let ArchiveReviewOwner::Approval(approval) = &self.approval else {
            return None;
        };
        match approval.as_ref().err()?.reason() {
            crate::archive_approval::ArchiveApprovalError::Cleanup(cleanup) => Some(*cleanup),
            _ => None,
        }
    }
    pub fn approval_native_error(&self) -> Option<&std::io::Error> {
        let ArchiveReviewOwner::Approval(approval) = &self.approval else {
            return None;
        };
        match approval.as_ref().err()?.reason() {
            crate::archive_approval::ArchiveApprovalError::Native(error) => Some(error),
            _ => None,
        }
    }
    pub fn archive_journal_root(&self) -> &std::sync::Arc<solstone_core_journal_io::JournalRoot> {
        match &self.approval {
            ArchiveReviewOwner::Finishing(owner) => owner.carrier().journal_root(),
            ArchiveReviewOwner::Pending { _read: read } => read.journal_root(),
            ArchiveReviewOwner::Approval(Ok(batch)) => batch.journal_root(),
            ArchiveReviewOwner::Approval(Err(failure)) => failure.journal_root(),
        }
    }
    pub fn archive_activity(
        &self,
    ) -> &std::sync::Arc<solstone_core_journal_io::activity::ActivityLease> {
        match &self.approval {
            ArchiveReviewOwner::Finishing(owner) => owner.carrier().activity(),
            ArchiveReviewOwner::Pending { _read: read } => read.activity(),
            ArchiveReviewOwner::Approval(Ok(batch)) => batch.activity(),
            ArchiveReviewOwner::Approval(Err(failure)) => failure.activity(),
        }
    }
    pub fn archive_expected_marks(&self) -> Option<&[Mark]> {
        match &self.approval {
            ArchiveReviewOwner::Finishing(owner) => Some(owner.expected_marks()),
            ArchiveReviewOwner::Pending { .. } => None,
            ArchiveReviewOwner::Approval(Ok(batch)) => Some(batch.expected_marks()),
            ArchiveReviewOwner::Approval(Err(failure)) => Some(failure.expected_marks()),
        }
    }
    pub fn archive_reads(&self) -> impl Iterator<Item = ArchiveReadState<'_>> {
        match &self.approval {
            ArchiveReviewOwner::Finishing(owner) => owner.read_states(),
            ArchiveReviewOwner::Pending { .. } => {
                crate::archive_approval::ArchiveReadStates::empty()
            }
            ArchiveReviewOwner::Approval(Ok(batch)) => batch.read_states(),
            ArchiveReviewOwner::Approval(Err(failure)) => failure.read_states(),
        }
    }
}

/// Confirm the selected typed offers using the original admitted archive operation.
/// Refusal retains that operation; successful readback remains subject to fresh
/// processing, catalog, consent and native checks at the removal door.
pub fn remove_reviewed_with_archive<'a>(
    checked: crate::review_context::CheckedReviewContext,
    read: solstone_core_backup::archive_readback::ReadOperation<'a>,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> ReviewedArchiveRemoval<'a> {
    #[cfg(any(unix, windows))]
    let selected = preflight_archive_review(&checked, &read);
    #[cfg(not(any(unix, windows)))]
    let selected: Result<Vec<Mark>, ReviewedRemovalRefusal> =
        Err(ReviewedRemovalRefusal::Unsupported);
    let selected = match selected {
        Ok(selected) => selected,
        Err(reason) => {
            return finish_archive_review_owner(
                checked,
                ArchiveReviewOwner::Pending {
                    _read: Box::new(read),
                },
                ReviewedExecution::new(),
                Err(reason),
            );
        }
    };
    let carrier = std::sync::Arc::clone(read.carrier());
    let approval = crate::archive_approval::create_archive_approval_batch(read, carrier, &selected);
    remove_reviewed_approved(checked, approval, today, now)
}

#[cfg(any(unix, windows))]
fn preflight_archive_review(
    checked: &crate::review_context::CheckedReviewContext,
    read: &solstone_core_backup::archive_readback::ReadOperation<'_>,
) -> Result<Vec<Mark>, ReviewedRemovalRefusal> {
    use crate::review_context::{check_marks, read_at_health, revalidate_health};
    use solstone_core_journal_io::media::MediaDirectory;
    if !valid_reviewed_coordinates(checked.marks())
        || !std::sync::Arc::ptr_eq(checked.root_arc(), read.journal_root())
    {
        return Err(ReviewedRemovalRefusal::InvalidSelection);
    }
    read.revalidate_admission()
        .map_err(ReviewedRemovalRefusal::Readback)?;
    revalidate_health(checked.root(), checked.health())
        .map_err(crate::mark_mutation::BoundMutationRefusal::from)?;
    let current = read_at_health(checked.health())
        .map_err(crate::mark_mutation::BoundMutationRefusal::from)?
        .ok_or(ReviewedRemovalRefusal::Changed)?;
    check_marks(
        &current,
        checked.marks(),
        &checked.root().identity().observation_id(),
    )
    .map_err(crate::mark_mutation::BoundMutationRefusal::from)?;
    let selected = checked
        .marks()
        .iter()
        .filter(|mark| mark.offload.is_some())
        .cloned()
        .collect::<Vec<_>>();
    if selected.is_empty()
        || selected.iter().any(|mark| {
            mark.class != RemovalClass::OffloadRawRelease
                || mark.offload.as_ref().is_none_or(|offer| {
                    offer.proof.repository_id != read.repository_id()
                        || offer.proof.target != mark.target
                        || offer.proof.validate().is_err()
                })
        })
    {
        return Err(ReviewedRemovalRefusal::InvalidSelection);
    }
    let maximum = selected
        .iter()
        .filter_map(|mark| mark.offload.as_ref())
        .flat_map(|offer| &offer.proof.files)
        .map(|file| file.bytes)
        .max()
        .ok_or(ReviewedRemovalRefusal::InvalidSelection)?;
    let config_directory = MediaDirectory::open(
        std::sync::Arc::clone(read.journal_root()),
        Path::new("config"),
    )?;
    let (config, entry) = solstone_core_journal_config::read_journal_config_observed_in_directory(
        &config_directory,
        REVIEW_MAX_CONFIG,
    )
    .map_err(ReviewedRemovalRefusal::Configuration)?;
    let metadata = (crate::review_context::MAX_REGISTER_BYTES as u64)
        .checked_add(REVIEW_MAX_CONFIG as u64)
        .ok_or(ReviewedRemovalRefusal::OffloadFloorRequired)?;
    let required = read
        .offload_required_bytes(maximum, metadata)
        .map_err(|error| match error {
            solstone_core_backup::archive_readback::ReadbackError::Admission(
                solstone_core_backup::archive_admission::AdmissionError::ConfigurationUnavailable,
            ) => ReviewedRemovalRefusal::OffloadFloorRequired,
            other => ReviewedRemovalRefusal::Readback(other),
        })?;
    let parent = MediaDirectory::open(std::sync::Arc::clone(read.journal_root()), Path::new(""))?;
    let available = parent.available_bytes()?;
    if available < required {
        return Err(ReviewedRemovalRefusal::InsufficientSpace {
            available,
            required,
        });
    }
    let (current_config, current_entry) =
        solstone_core_journal_config::read_journal_config_observed_in_directory(
            &config_directory,
            REVIEW_MAX_CONFIG,
        )
        .map_err(ReviewedRemovalRefusal::Configuration)?;
    if current_config != config || current_entry != entry {
        return Err(ReviewedRemovalRefusal::Changed);
    }
    parent.revalidate()?;
    read.revalidate_admission()
        .map_err(ReviewedRemovalRefusal::Readback)?;
    revalidate_health(checked.root(), checked.health())
        .map_err(crate::mark_mutation::BoundMutationRefusal::from)?;
    Ok(selected)
}

pub(crate) fn remove_reviewed_approved<'a>(
    checked: crate::review_context::CheckedReviewContext,
    approval: Result<
        crate::archive_approval::ArchiveApprovalBatch<'a>,
        crate::archive_approval::ArchiveApprovalFailure<'a>,
    >,
    today: NaiveDate,
    now: DateTime<Utc>,
) -> ReviewedArchiveRemoval<'a> {
    #[cfg(any(unix, windows))]
    {
        remove_reviewed_approved_with(checked, approval, today, now, |_, _| {})
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (today, now);
        finish_archive_review(
            checked,
            approval,
            ReviewedExecution::new(),
            Err(ReviewedRemovalRefusal::Unsupported),
        )
    }
}

#[cfg(any(unix, windows))]
fn remove_reviewed_approved_with<'a>(
    checked: crate::review_context::CheckedReviewContext,
    approval: Result<
        crate::archive_approval::ArchiveApprovalBatch<'a>,
        crate::archive_approval::ArchiveApprovalFailure<'a>,
    >,
    today: NaiveDate,
    now: DateTime<Utc>,
    checkpoint: impl FnMut(ReviewedCheckpoint, &Path),
) -> ReviewedArchiveRemoval<'a> {
    let mut execution = ReviewedExecution::new();
    let result = match &approval {
        Ok(batch) => execute_reviewed_mode(
            &checked,
            today,
            now,
            &mut execution,
            Some(batch.view()),
            checkpoint,
        ),
        Err(_) => Err(ReviewedRemovalRefusal::ArchiveApproval),
    };
    finish_archive_review(checked, approval, execution, result)
}

#[cfg(all(test, unix, feature = "full-tests"))]
pub(crate) fn remove_reviewed_approved_with_test_hook<'a>(
    checked: crate::review_context::CheckedReviewContext,
    approval: Result<
        crate::archive_approval::ArchiveApprovalBatch<'a>,
        crate::archive_approval::ArchiveApprovalFailure<'a>,
    >,
    today: NaiveDate,
    now: DateTime<Utc>,
    checkpoint: impl FnMut(ReviewedCheckpoint, &Path),
) -> ReviewedArchiveRemoval<'a> {
    remove_reviewed_approved_with(checked, approval, today, now, checkpoint)
}

fn finish_archive_review<'a>(
    checked: crate::review_context::CheckedReviewContext,
    approval: Result<
        crate::archive_approval::ArchiveApprovalBatch<'a>,
        crate::archive_approval::ArchiveApprovalFailure<'a>,
    >,
    execution: ReviewedExecution,
    result: Result<(), ReviewedRemovalRefusal>,
) -> ReviewedArchiveRemoval<'a> {
    finish_archive_review_owner(
        checked,
        ArchiveReviewOwner::Approval(approval),
        execution,
        result,
    )
}

fn finish_archive_review_owner<'a>(
    checked: crate::review_context::CheckedReviewContext,
    approval: ArchiveReviewOwner<'a>,
    mut execution: ReviewedExecution,
    result: Result<(), ReviewedRemovalRefusal>,
) -> ReviewedArchiveRemoval<'a> {
    finish_archive_execution(&mut execution, result);
    ReviewedArchiveRemoval {
        checked,
        approval,
        execution,
    }
}

fn finish_archive_execution(
    execution: &mut ReviewedExecution,
    result: Result<(), ReviewedRemovalRefusal>,
) {
    if let Err(error) = result {
        execution.refusal = Some(error);
        reviewed_mark_unsaved_deletions(&mut execution.outcome);
        execution.outcome.halted = Some(RunHalt {
            reason: if execution.attempts.is_empty() {
                REVIEW_REFUSED
            } else {
                REVIEW_HALTED
            }
            .to_owned(),
        });
    }
}

fn reviewed_mark_unsaved_deletions(outcome: &mut Outcome) {
    for row in &mut outcome.targets {
        if let Some(removed) = row.removed.first() {
            row.post_commit_failure = Some(crate::receipt::PostCommitFailure {
                entry: removed.as_str().to_owned(),
                reason: REVIEW_POST_COMMIT.to_owned(),
            });
        }
    }
}

#[cfg(windows)]
fn open_windows_directory_path(
    root: &std::sync::Arc<solstone_core_journal_io::JournalRoot>,
    relative: &Path,
) -> Result<solstone_core_journal_io::WindowsFlatDirectory, ReviewedRemovalRefusal> {
    use solstone_core_journal_io::open_windows_flat_directory_bound;
    let mut opened: Option<solstone_core_journal_io::WindowsFlatDirectory> = None;
    let mut diagnostic = root.canonical_path().to_path_buf();
    for component in relative.components() {
        let next = match &opened {
            Some(directory) => {
                open_windows_flat_directory_bound(directory, component.as_os_str(), &diagnostic)
            }
            None => {
                open_windows_flat_directory_bound(root.as_ref(), component.as_os_str(), &diagnostic)
            }
        }
        .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?
        .ok_or(ReviewedRemovalRefusal::Changed)?;
        opened = Some(next);
        diagnostic.push(component);
    }
    opened.ok_or(ReviewedRemovalRefusal::Changed)
}

#[cfg(any(unix, windows))]
const REVIEW_MAX_ENTRIES: usize = 65_536;
#[cfg(any(unix, windows))]
const REVIEW_MAX_CONFIG: usize = 1024 * 1024;

#[cfg(any(unix, windows))]
pub(crate) struct ReviewedPolicy {
    root: std::sync::Arc<solstone_core_journal_io::JournalRoot>,
    directory: std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
    #[cfg(unix)]
    parent: solstone_core_journal_io::FlatDirectory,
    #[cfg(windows)]
    parent: solstone_core_journal_io::WindowsFlatDirectory,
    lock: solstone_core_journal_io::BoundParentLock,
    read: solstone_core_journal_config::JournalConfigRead,
    entry: Option<solstone_core_journal_io::media::MediaEntry>,
    policy: Policy,
}

#[cfg(any(unix, windows))]
impl ReviewedPolicy {
    pub(crate) fn capture(
        root: &std::sync::Arc<solstone_core_journal_io::JournalRoot>,
    ) -> Result<Self, ReviewedRemovalRefusal> {
        use solstone_core_journal_io::media::MediaDirectory;
        use solstone_core_journal_io::{
            acquire_existing_parent_lock_bound, open_existing_parent_lock_bound,
        };
        use std::ffi::OsStr;
        use std::time::Duration;
        let directory = MediaDirectory::open(std::sync::Arc::clone(root), Path::new("config"))?;
        #[cfg(unix)]
        let parent = {
            let parent = solstone_core_journal_io::FlatDirectory::open(root, Path::new("config"))?;
            if parent.identity() != directory.object_identity() {
                return Err(ReviewedRemovalRefusal::Changed);
            }
            parent
        };
        #[cfg(windows)]
        let parent = {
            let parent = open_windows_directory_path(root, Path::new("config"))?;
            if parent.object_identity() != directory.object_identity() {
                return Err(ReviewedRemovalRefusal::Changed);
            }
            parent
        };
        let name = OsStr::new("journal.json.lock");
        let lock =
            match open_existing_parent_lock_bound(&parent, name, Duration::ZERO, Duration::ZERO)
                .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?
            {
                Some(lock) => lock,
                None => acquire_existing_parent_lock_bound(
                    &parent,
                    name,
                    Duration::ZERO,
                    Duration::ZERO,
                )
                .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?,
            };
        lock.revalidate_bound(&parent, name)
            .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?;
        let (read, entry) =
            solstone_core_journal_config::read_journal_config_observed_in_directory(
                &directory,
                REVIEW_MAX_CONFIG,
            )
            .map_err(ReviewedRemovalRefusal::Configuration)?;
        let config = read
            .config
            .as_ref()
            .ok_or(ReviewedRemovalRefusal::Changed)?;
        let policy = crate::policy::policy_from_journal_config(config);
        let admitted = Self {
            root: std::sync::Arc::clone(root),
            directory,
            parent,
            lock,
            read,
            entry,
            policy,
        };
        admitted.revalidate()?;
        Ok(admitted)
    }
    pub(crate) fn policy(&self) -> &Policy {
        &self.policy
    }

    pub(crate) fn observation(
        &self,
    ) -> (
        &solstone_core_journal_config::JournalConfigRead,
        &Option<solstone_core_journal_io::media::MediaEntry>,
    ) {
        (&self.read, &self.entry)
    }

    pub(crate) fn revalidate(&self) -> Result<(), ReviewedRemovalRefusal> {
        self.root
            .revalidate_canonical_binding()
            .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?;
        self.directory.revalidate()?;
        #[cfg(unix)]
        {
            self.parent.revalidate()?;
            let current_parent =
                solstone_core_journal_io::FlatDirectory::open(&self.root, Path::new("config"))?;
            if current_parent.identity() != self.parent.identity() {
                return Err(ReviewedRemovalRefusal::Changed);
            }
        }
        #[cfg(windows)]
        {
            self.parent
                .revalidate_bound()
                .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?;
            let current_parent = open_windows_directory_path(&self.root, Path::new("config"))?;
            if current_parent.object_identity() != self.directory.object_identity() {
                return Err(ReviewedRemovalRefusal::Changed);
            }
        }
        self.lock
            .revalidate_bound(&self.parent, std::ffi::OsStr::new("journal.json.lock"))
            .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?;
        let (read, entry) =
            solstone_core_journal_config::read_journal_config_observed_in_directory(
                &self.directory,
                REVIEW_MAX_CONFIG,
            )
            .map_err(ReviewedRemovalRefusal::Configuration)?;
        if read != self.read || entry != self.entry {
            return Err(ReviewedRemovalRefusal::Changed);
        }
        self.directory.revalidate()?;
        Ok(())
    }
}

#[cfg(any(unix, windows))]
struct ReviewedGroup {
    target: crate::Target,
    layout: solstone_core_journal_io::paths::SegmentLayout,
    directory: std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
    parent: std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
    #[cfg(unix)]
    events: solstone_core_journal_io::FlatDirectory,
    #[cfg(unix)]
    health: solstone_core_journal_io::FlatDirectory,
    #[cfg(windows)]
    events: solstone_core_journal_io::WindowsFlatDirectory,
    #[cfg(windows)]
    health: solstone_core_journal_io::WindowsFlatDirectory,
    scan: crate::scan::CheckedSegment,
    files: BTreeMap<String, (ProvenRaw, solstone_core_journal_io::media::MediaObservation)>,
    refusals: BTreeMap<String, String>,
    archive_refusals: BTreeMap<String, String>,
    policy_names: BTreeSet<String>,
    owner_deletion: solstone_core_segment::RetainedOwnerDeletion,
    deleted: BTreeSet<String>,
}

#[cfg(any(unix, windows))]
impl ReviewedGroup {
    fn relative(&self) -> String {
        crate::layout::segment_rel_with_layout(
            &self.target.day,
            &self.target.stream,
            &self.target.dir,
            self.layout,
        )
    }
}

#[cfg(any(unix, windows))]
fn reviewed_layout(mark: &Mark) -> solstone_core_journal_io::paths::SegmentLayout {
    use solstone_core_journal_io::paths::{DEFAULT_STREAM, SegmentLayout};
    mark.offload.as_ref().map_or_else(
        || {
            if mark.target.stream == DEFAULT_STREAM {
                SegmentLayout::Direct
            } else {
                SegmentLayout::Named
            }
        },
        |offer| offer.proof.layout,
    )
}

#[cfg(any(unix, windows))]
fn reviewed_relative(mark: &Mark, name: &str) -> String {
    crate::layout::content_rel_with_layout(
        &mark.target.day,
        &mark.target.stream,
        &mark.target.dir,
        name,
        reviewed_layout(mark),
    )
}

#[cfg(any(unix, windows))]
fn reviewed_scan(
    directory: &solstone_core_journal_io::media::MediaDirectory,
) -> std::io::Result<crate::scan::CheckedSegment> {
    crate::scan::scan_segment_bound(
        directory,
        &ClosedHandlerSet,
        &JournalMedia,
        REVIEW_MAX_ENTRIES,
    )
}

#[cfg(any(unix, windows))]
fn revalidate_reviewed_group(group: &ReviewedGroup) -> Result<(), ReviewedRemovalRefusal> {
    group.directory.revalidate()?;
    group.parent.revalidate()?;
    if group
        .owner_deletion
        .state()
        .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?
        != solstone_core_segment::OwnerDeletionState::Live
    {
        return Err(ReviewedRemovalRefusal::Changed);
    }
    let current = reviewed_scan(&group.directory)?;
    let expected = group
        .scan
        .files()
        .iter()
        .filter(|file| !group.deleted.contains(file.name.as_str()))
        .collect::<Vec<_>>();
    if expected.len() != current.files().len()
        || expected.iter().zip(current.files()).any(|(a, b)| {
            a.name != b.name
                || a.size != b.size
                || a.sidecar.record != b.sidecar.record
                || a.sidecar.has_analysis_row != b.sidecar.has_analysis_row
                || group.scan.bound_entry(&a.name) != current.bound_entry(&b.name)
        })
    {
        return Err(ReviewedRemovalRefusal::Changed);
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn reviewed_candidates(
    scan: &crate::scan::CheckedSegment,
    policy: &Policy,
    target: &crate::Target,
    today: NaiveDate,
    now: DateTime<Utc>,
    purpose: DecisionPurpose,
) -> (Vec<ProvenRaw>, BTreeMap<String, String>) {
    let (empty, ordinary) = partition_empty_audio(scan.files().to_vec(), &ClosedHandlerSet);
    let decide = |side: &[FoundContent]| match crate::grouped::decide_side(
        side,
        &ClosedHandlerSet,
        &JournalMedia,
        policy,
        target,
        today,
        now,
        purpose,
    ) {
        SideDecision::NoMedia => SideOutcome::None,
        SideDecision::Kept(verdict) => SideOutcome::Policy(verdict),
        SideDecision::Unproven(blockers) => SideOutcome::Held(blockers),
        SideDecision::Eligible { proven, .. } => SideOutcome::Proved(proven),
    };
    let empty_decision = decide(&empty);
    let ordinary_decision = decide(&ordinary);
    let mut refusals = BTreeMap::new();
    record_side_refusal(&empty, &empty_decision, &mut refusals);
    record_side_refusal(&ordinary, &ordinary_decision, &mut refusals);
    (ready_union(empty_decision, ordinary_decision), refusals)
}

#[cfg(any(unix, windows))]
fn valid_reviewed_coordinates(marks: &[Mark]) -> bool {
    use crate::content::{ContentName, MediaClassifier};
    let plain = |s: &str| {
        !s.is_empty()
            && s != "."
            && s != ".."
            && !s.chars().any(|c| c.is_control() || c == '/' || c == '\\')
    };
    !marks.is_empty()
        && marks.iter().all(|mark| {
            // An offload mark without its archive offer has no backup proof
            // behind it; it must never fall through to the policy decision.
            (mark.class == RemovalClass::PolicyRawRelease
                || (mark.class == RemovalClass::OffloadRawRelease && mark.offload.is_some()))
                && mark.target.day.len() == 8
                && mark.target.day.bytes().all(|b| b.is_ascii_digit())
                && NaiveDate::parse_from_str(&mark.target.day, "%Y%m%d").is_ok()
                && plain(&mark.target.stream)
                && plain(&mark.target.dir)
                && mark.proposal.names.iter().all(|name| {
                    plain(name)
                        && ContentName::new(name)
                            .is_some_and(|name| JournalMedia.is_owner_media(&name))
                })
        })
}

#[cfg(any(unix, windows))]
fn valid_reviewed_selection(marks: &[Mark]) -> bool {
    valid_reviewed_coordinates(marks) && marks.iter().all(|mark| mark.offload.is_none())
}

#[cfg(any(unix, windows))]
fn reviewed_admission(
    archive: Option<crate::archive_approval::ArchiveApprovalRef<'_, '_>>,
    execution: &ReviewedExecution,
) -> Result<(), ReviewedRemovalRefusal> {
    if let Some(archive) = archive {
        archive
            .revalidate_admission()
            .map_err(|_| ReviewedRemovalRefusal::ArchiveApproval)
    } else {
        execution
            .carrier
            .as_ref()
            .ok_or(ReviewedRemovalRefusal::Changed)?
            .revalidate()
            .map_err(ReviewedRemovalRefusal::Io)
    }
}

#[cfg(any(unix, windows))]
fn reviewed_archive_processing<'a>(
    archive: crate::archive_approval::ArchiveApprovalRef<'_, '_>,
    mark: &'a Mark,
    group: &ReviewedGroup,
    name: &str,
) -> Result<&'a crate::archive_proof::ArchiveFileProof, ReviewedRemovalRefusal> {
    if !archive.expected_marks().contains(mark) {
        return Err(ReviewedRemovalRefusal::InvalidSelection);
    }
    archive
        .revalidate_admission()
        .map_err(|_| ReviewedRemovalRefusal::ArchiveApproval)?;
    revalidate_reviewed_group(group)?;
    let offer = mark
        .offload
        .as_ref()
        .ok_or(ReviewedRemovalRefusal::InvalidSelection)?;
    let file = offer
        .proof
        .files
        .iter()
        .find(|file| file.name == name)
        .ok_or(ReviewedRemovalRefusal::InvalidSelection)?;
    let found = group
        .scan
        .files()
        .iter()
        .find(|file| file.name.as_str() == name)
        .ok_or(ReviewedRemovalRefusal::Changed)?;
    let processing = crate::archive_proof::AcceptedProcessingObservation::capture(
        &ClosedHandlerSet,
        &mark.target,
        &found.name,
        found.size,
        &found.sidecar,
    )
    .map_err(|_| ReviewedRemovalRefusal::Changed)?;
    if offer.proof.target != group.target
        || offer.proof.layout != group.layout
        || offer.proof.journal_id != archive.journal_root().identity().observation_id()
        || file.bytes != found.size
        || file.processing != processing
    {
        return Err(ReviewedRemovalRefusal::Changed);
    }
    Ok(file)
}

#[cfg(any(unix, windows))]
fn reviewed_archive_file(
    archive: crate::archive_approval::ArchiveApprovalRef<'_, '_>,
    mark: &Mark,
    group: &ReviewedGroup,
    name: &str,
) -> Result<(), ReviewedRemovalRefusal> {
    let file = reviewed_archive_processing(archive, mark, group, name)?;
    let (proven, observed) = group
        .files
        .get(name)
        .ok_or(ReviewedRemovalRefusal::Changed)?;
    if file.bytes != proven.size()
        || file.bytes != observed.digest.size
        || file.sha256 != observed.digest.sha256
    {
        return Err(ReviewedRemovalRefusal::Changed);
    }
    let offer = mark
        .offload
        .as_ref()
        .ok_or(ReviewedRemovalRefusal::InvalidSelection)?;
    let catalog = crate::archive_catalog::read_complete_catalog(archive.journal_root())
        .map_err(ReviewedRemovalRefusal::Catalog)?;
    if catalog.active_witness(&offer.proof, file).is_none() {
        return Err(ReviewedRemovalRefusal::Changed);
    }
    Ok(())
}

/// Only the retained current verifier can mint the ordinary-age exception.
#[cfg(any(unix, windows))]
pub(crate) struct ArchiveRelease<'a, 'op> {
    archive: crate::archive_approval::ArchiveApprovalRef<'a, 'op>,
    register: &'a crate::mark_mutation::CheckedMarkGuard<'a, 'op>,
    policy: &'a ReviewedPolicy,
    group: &'a ReviewedGroup,
    mark: &'a Mark,
    name: &'a str,
    today: NaiveDate,
    now: DateTime<Utc>,
}

#[cfg(any(unix, windows))]
impl ArchiveRelease<'_, '_> {
    pub(crate) fn revalidate(&self) -> Result<(), ReviewedRemovalRefusal> {
        self.register.revalidate()?;
        self.policy.revalidate()?;
        reviewed_archive_file(self.archive, self.mark, self.group, self.name)?;
        let current = reviewed_scan(&self.group.directory)?;
        let (ready, _) = reviewed_candidates(
            &current,
            &self.policy.policy,
            &self.group.target,
            self.today,
            self.now,
            DecisionPurpose::OffloadProposal,
        );
        if !ready.iter().any(|file| file.name() == self.name) {
            return Err(ReviewedRemovalRefusal::Changed);
        }
        Ok(())
    }
    pub(crate) fn into_parts(
        self,
    ) -> Result<
        (
            std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
            ProvenRaw,
            solstone_core_journal_io::media::MediaObservation,
            String,
        ),
        ReviewedRemovalRefusal,
    > {
        let (proven, observed) = self
            .group
            .files
            .get(self.name)
            .cloned()
            .ok_or(ReviewedRemovalRefusal::Changed)?;
        Ok((
            self.group.directory.clone(),
            proven,
            observed,
            reviewed_relative(self.mark, self.name),
        ))
    }
}

/// Minted only by the current ordinary executor after its full retained checks.
#[cfg(any(unix, windows))]
pub(crate) struct OrdinaryRelease {
    source: std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
    proven: ProvenRaw,
    observation: solstone_core_journal_io::media::MediaObservation,
}

#[cfg(any(unix, windows))]
impl OrdinaryRelease {
    pub(crate) fn into_parts(
        self,
    ) -> (
        std::sync::Arc<solstone_core_journal_io::media::MediaDirectory>,
        ProvenRaw,
        solstone_core_journal_io::media::MediaObservation,
    ) {
        (self.source, self.proven, self.observation)
    }
}

#[cfg(any(unix, windows))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ReviewedCheckpoint {
    Hashed,
    BeforeRaw,
    AfterRaw,
    BeforeBookkeeping,
    BeforeFinalCas,
}

#[cfg(any(unix, windows))]
fn execute_reviewed(
    checked: &crate::review_context::CheckedReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
    execution: &mut ReviewedExecution,
) -> Result<(), ReviewedRemovalRefusal> {
    execute_reviewed_with(checked, today, now, execution, |_, _| {})
}

#[cfg(any(unix, windows))]
fn execute_reviewed_with(
    checked: &crate::review_context::CheckedReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
    execution: &mut ReviewedExecution,
    checkpoint: impl FnMut(ReviewedCheckpoint, &Path),
) -> Result<(), ReviewedRemovalRefusal> {
    execute_reviewed_mode(checked, today, now, execution, None, checkpoint)
}

#[cfg(any(unix, windows))]
fn execute_reviewed_mode(
    checked: &crate::review_context::CheckedReviewContext,
    today: NaiveDate,
    now: DateTime<Utc>,
    execution: &mut ReviewedExecution,
    archive: Option<crate::archive_approval::ArchiveApprovalRef<'_, '_>>,
    mut checkpoint: impl FnMut(ReviewedCheckpoint, &Path),
) -> Result<(), ReviewedRemovalRefusal> {
    use crate::mark_mutation::CheckedMarkGuard;
    use solstone_core_journal_io::activity::ActivityLease;
    use solstone_core_journal_io::media::{
        Cleanup, Confirmation, MediaDirectory, MediaStage, OriginalName, RemovalEffect,
        RootActivityCarrier,
    };
    use solstone_core_segment::output_attempt::OutputClaims;
    use std::ffi::OsStr;
    use std::sync::Arc;
    use std::time::Duration;

    if !valid_reviewed_coordinates(checked.marks())
        || (archive.is_none() && !valid_reviewed_selection(checked.marks()))
    {
        return Err(ReviewedRemovalRefusal::InvalidSelection);
    }
    let root = Arc::clone(checked.root_arc());
    let activity = if let Some(archive) = archive {
        if !Arc::ptr_eq(&root, archive.journal_root())
            || checked
                .marks()
                .iter()
                .filter(|m| m.offload.is_some())
                .ne(archive.expected_marks())
        {
            return Err(ReviewedRemovalRefusal::InvalidSelection);
        }
        archive
            .revalidate_admission()
            .map_err(|_| ReviewedRemovalRefusal::ArchiveApproval)?;
        Arc::clone(archive.activity())
    } else {
        let activity = ActivityLease::try_acquire(Arc::clone(&root))
            .map_err(ReviewedRemovalRefusal::Activity)?;
        // As at archive admission: first recover what an ended owner left
        // behind, then let the fresh census decide.
        recover_interrupted_removals(&activity);
        crate::restore_intent::recover_abandoned_restores(&activity);
        solstone_core_journal_io::media::retire_orphan_root_carriers(&activity);
        let obligations =
            solstone_core_journal_io::activity_obligations::observe_obligations(&root)
                .map_err(ReviewedRemovalRefusal::Census)?;
        if !obligations.entries().is_empty() {
            return Err(ReviewedRemovalRefusal::Obligations(obligations));
        }
        match RootActivityCarrier::prepare(Arc::clone(&activity)) {
            Ok(carrier) => execution.carrier = Some(carrier),
            Err(failure) => {
                execution.carrier_failure = Some(failure);
                return Err(ReviewedRemovalRefusal::CarrierPreparation);
            }
        }
        activity
    };
    let acquire_register = || {
        if let Some(archive) = archive {
            CheckedMarkGuard::acquire_archive(checked, archive)
        } else {
            CheckedMarkGuard::acquire(checked)
        }
    };
    let mut targets = checked
        .marks()
        .iter()
        .map(|m| (m.target.clone(), reviewed_layout(m)))
        .collect::<Vec<_>>();
    targets.sort_by_key(|(t, layout)| {
        crate::layout::segment_rel_with_layout(&t.day, &t.stream, &t.dir, *layout)
    });
    targets.dedup();
    let options = LockOptions {
        timeout: Duration::ZERO,
        ..LockOptions::default()
    };
    let mut claims = Vec::new();
    let mut directories = Vec::new();
    for (target, layout) in &targets {
        let relative = crate::layout::segment_rel_with_layout(
            &target.day,
            &target.stream,
            &target.dir,
            *layout,
        );
        let directory = MediaDirectory::open(Arc::clone(&root), Path::new(&relative))?;
        let path = root
            .canonical_path()
            .join(&relative)
            .components()
            .collect::<std::path::PathBuf>();
        let claim = OutputClaims::discover(&path, options)?;
        if claim.segment_identity(&path)? != directory.object_identity() {
            return Err(ReviewedRemovalRefusal::Changed);
        }
        claims.push(claim);
        directories.push(directory);
    }
    let mut commits = Vec::new();
    for ((target, layout), claim) in targets.iter().zip(&claims) {
        let relative = crate::layout::segment_rel_with_layout(
            &target.day,
            &target.stream,
            &target.dir,
            *layout,
        );
        let path = root
            .canonical_path()
            .join(relative)
            .components()
            .collect::<std::path::PathBuf>();
        commits.push(claim.commit(&path)?);
    }
    let policy = ReviewedPolicy::capture(&root)?;
    let mut groups = Vec::new();
    for ((target, layout), directory) in targets.into_iter().zip(directories) {
        let parent = MediaDirectory::open(
            Arc::clone(&root),
            Path::new(&crate::layout::stream_rel_with_layout(
                &target.day,
                &target.stream,
                layout,
            )),
        )?;
        #[cfg(unix)]
        let events = solstone_core_journal_io::FlatDirectory::open(
            &root,
            Path::new(&crate::layout::segment_rel_with_layout(
                &target.day,
                &target.stream,
                &target.dir,
                layout,
            )),
        )?;
        #[cfg(windows)]
        let events = open_windows_directory_path(
            &root,
            Path::new(&crate::layout::segment_rel_with_layout(
                &target.day,
                &target.stream,
                &target.dir,
                layout,
            )),
        )?;
        #[cfg(unix)]
        let health = solstone_core_journal_io::FlatDirectory::open(
            &root,
            Path::new(&crate::layout::day_health_rel(&target.day)),
        )?;
        #[cfg(windows)]
        let health = open_windows_directory_path(
            &root,
            Path::new(&crate::layout::day_health_rel(&target.day)),
        )?;
        #[cfg(unix)]
        if events.identity() != directory.object_identity() {
            return Err(ReviewedRemovalRefusal::Changed);
        }
        #[cfg(windows)]
        if events.object_identity() != directory.object_identity() {
            return Err(ReviewedRemovalRefusal::Changed);
        }
        let scan = reviewed_scan(&directory)?;
        let (mut ready, refusals) = reviewed_candidates(
            &scan,
            &policy.policy,
            &target,
            today,
            now,
            DecisionPurpose::PolicyRelease,
        );
        let policy_names = ready
            .iter()
            .map(|file| file.name().to_owned())
            .collect::<BTreeSet<_>>();
        let typed_names = checked
            .marks()
            .iter()
            .filter(|m| m.target == target && reviewed_layout(m) == layout && m.offload.is_some())
            .flat_map(|m| m.proposal.names.iter().cloned())
            .collect::<BTreeSet<_>>();
        let (archive_ready, archive_refusals) = if archive.is_some() {
            reviewed_candidates(
                &scan,
                &policy.policy,
                &target,
                today,
                now,
                DecisionPurpose::OffloadProposal,
            )
        } else {
            (Vec::new(), BTreeMap::new())
        };
        for file in archive_ready
            .into_iter()
            .filter(|file| typed_names.contains(file.name()))
        {
            if !ready.iter().any(|prior| prior.name() == file.name()) {
                ready.push(file);
            }
        }
        // Every release, policy or archive, refuses a segment the owner deleted.
        let owner_deletion = solstone_core_segment::RetainedOwnerDeletion::open(
            Arc::clone(&parent),
            OsStr::new(&target.dir),
            Some(&directory),
            REVIEW_MAX_ENTRIES,
        )
        .map_err(|e| ReviewedRemovalRefusal::Io(std::io::Error::other(e)))?;
        let desired = checked
            .marks()
            .iter()
            .filter(|m| m.target == target && reviewed_layout(m) == layout)
            .flat_map(|m| m.proposal.names.iter().cloned())
            .collect::<BTreeSet<_>>();
        let mut files = BTreeMap::new();
        for proven in ready.into_iter().filter(|p| desired.contains(p.name())) {
            let name = crate::content::ContentName::new(proven.name())
                .ok_or(ReviewedRemovalRefusal::InvalidSelection)?;
            let entry = scan
                .bound_entry(&name)
                .ok_or(ReviewedRemovalRefusal::Changed)?;
            let before = directory
                .read_prefix(OsStr::new(proven.name()), 0)?
                .ok_or(ReviewedRemovalRefusal::Changed)?;
            if before.entry != *entry {
                return Err(ReviewedRemovalRefusal::Changed);
            }
            let observed = directory
                .observe_file(OsStr::new(proven.name()))?
                .ok_or(ReviewedRemovalRefusal::Changed)?;
            let after = directory
                .read_prefix(OsStr::new(proven.name()), 0)?
                .ok_or(ReviewedRemovalRefusal::Changed)?;
            if after.entry != *entry
                || observed.identity != entry.identity
                || observed.digest.size != proven.size()
            {
                return Err(ReviewedRemovalRefusal::Changed);
            }
            checkpoint(
                ReviewedCheckpoint::Hashed,
                &directory.diagnostic_path().join(proven.name()),
            );
            files.insert(proven.name().to_owned(), (proven, observed));
        }
        let group = ReviewedGroup {
            target,
            layout,
            directory,
            parent,
            events,
            health,
            scan,
            files,
            refusals,
            archive_refusals,
            policy_names,
            owner_deletion,
            deleted: BTreeSet::new(),
        };
        revalidate_reviewed_group(&group)?;
        if let Some(archive) = archive {
            for mark in checked.marks().iter().filter(|m| {
                m.offload.is_some()
                    && m.target == group.target
                    && reviewed_layout(m) == group.layout
            }) {
                for name in &mark.proposal.names {
                    if group
                        .scan
                        .files()
                        .iter()
                        .any(|file| file.name.as_str() == name)
                    {
                        reviewed_archive_processing(archive, mark, &group, name)?;
                    }
                    if group.files.contains_key(name) && !group.archive_refusals.contains_key(name)
                    {
                        reviewed_archive_file(archive, mark, &group, name)?;
                    }
                }
            }
        }
        groups.push(group);
    }
    for commit in &commits {
        commit.revalidate()?;
    }
    policy.revalidate()?;
    reviewed_admission(archive, execution)?;
    let mut attempted: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut stage_failed: BTreeMap<(usize, String), usize> = BTreeMap::new();
    let mut stop = false;
    // This guard ends before any event/marker bookkeeping or final reacquisition.
    {
        let register = acquire_register()?;
        for mark in checked.marks() {
            let group_index = groups
                .iter()
                .position(|g| g.target == mark.target && g.layout == reviewed_layout(mark))
                .ok_or(ReviewedRemovalRefusal::Changed)?;
            let group = groups
                .get_mut(group_index)
                .ok_or(ReviewedRemovalRefusal::Changed)?;
            let mut row = TargetOutcome {
                target: mark.target.clone(),
                removed: Vec::new(),
                not_removed: Vec::new(),
                post_commit_failure: None,
            };
            for name in &mark.proposal.names {
                if stop {
                    row.not_removed
                        .push(reviewed_not_removed(mark, name, REVIEW_HALTED, None));
                    continue;
                }
                if attempted.contains_key(&(group_index, name.clone()))
                    || stage_failed.contains_key(&(group_index, name.clone()))
                {
                    continue;
                }
                if !group.scan.files().iter().any(|f| f.name.as_str() == name) {
                    row.not_removed
                        .push(reviewed_not_removed(mark, name, NO_LONGER_PRESENT, None));
                    continue;
                }
                let refusals = if mark.offload.is_some() {
                    &group.archive_refusals
                } else {
                    &group.refusals
                };
                if (mark.offload.is_none() && !group.policy_names.contains(name))
                    || refusals.contains_key(name)
                {
                    row.not_removed.push(reviewed_not_removed(
                        mark,
                        name,
                        refusals.get(name).map_or(REVIEW_REFUSED, String::as_str),
                        None,
                    ));
                    continue;
                }
                let Some((proven, observed)) = group.files.get(name).cloned() else {
                    row.not_removed.push(reviewed_not_removed(
                        mark,
                        name,
                        group
                            .refusals
                            .get(name)
                            .map_or(REVIEW_REFUSED, String::as_str),
                        None,
                    ));
                    continue;
                };
                checkpoint(
                    ReviewedCheckpoint::BeforeRaw,
                    &group.directory.diagnostic_path().join(name),
                );
                let recheck = (|| {
                    register.revalidate()?;
                    policy.revalidate()?;
                    reviewed_admission(archive, execution)?;
                    revalidate_reviewed_group(group)
                })();
                if let Err(error) = recheck {
                    execution.refusal = Some(error);
                    stop = true;
                    row.not_removed
                        .push(reviewed_not_removed(mark, name, REVIEW_REFUSED, None));
                    continue;
                }
                let archive_release = if mark.offload.is_some() {
                    let archive = archive.ok_or(ReviewedRemovalRefusal::InvalidSelection)?;
                    let release = ArchiveRelease {
                        archive,
                        register: &register,
                        policy: &policy,
                        group,
                        mark,
                        name,
                        today,
                        now,
                    };
                    if let Err(error) = release.revalidate() {
                        execution.refusal = Some(error);
                        row.not_removed.push(reviewed_not_removed(
                            mark,
                            name,
                            REVIEW_REFUSED,
                            None,
                        ));
                        stop = true;
                        continue;
                    }
                    Some(release)
                } else {
                    None
                };
                let stage =
                    match MediaStage::create(Arc::clone(&group.parent), Arc::clone(&activity)) {
                        Ok(stage) => stage,
                        Err(failure) => {
                            let staged = failure.owned.as_ref().map(|stage| {
                                journal_rel(
                                    stage
                                        .diagnostic_path()
                                        .strip_prefix(root.canonical_path())
                                        .unwrap_or_else(|_| Path::new("")),
                                )
                            });
                            let index = execution.stage_failures.len();
                            execution.stage_failures.push(failure);
                            stage_failed.insert((group_index, name.clone()), index);
                            row.not_removed.push(reviewed_not_removed(
                                mark,
                                name,
                                REVIEW_UNCERTAIN,
                                staged,
                            ));
                            stop = true;
                            continue;
                        }
                    };
                let released = if let Some(release) = archive_release {
                    match crate::door::release_reviewed_archive(release, stage) {
                        Ok(released) => released,
                        Err(failure) => {
                            execution.refusal = Some(failure.reason);
                            execution.stage_failures.push(
                                solstone_core_journal_io::media::OwnedMediaFailure {
                                    error: std::io::Error::other(REVIEW_REFUSED),
                                    owned: Some(failure.stage),
                                },
                            );
                            stage_failed.insert(
                                (group_index, name.clone()),
                                execution.stage_failures.len().saturating_sub(1),
                            );
                            row.not_removed.push(reviewed_not_removed(
                                mark,
                                name,
                                REVIEW_REFUSED,
                                None,
                            ));
                            stop = true;
                            continue;
                        }
                    }
                } else {
                    crate::door::release_reviewed_ordinary(
                        OrdinaryRelease {
                            source: Arc::clone(&group.directory),
                            proven,
                            observation: observed,
                        },
                        stage,
                    )
                };
                let confirmed = released.removed.is_some();
                if let Some(removed) = released.removed {
                    row.removed.push(removed);
                    group.deleted.insert(name.clone());
                }
                let receipt = released.native.receipt();
                let unresolved = !matches!(
                    receipt.effect,
                    RemovalEffect::NotClaimed | RemovalEffect::Deleted
                ) || receipt.cleanup != Cleanup::Complete
                    || (receipt.effect == RemovalEffect::Deleted && !confirmed)
                    || receipt.original_name == OriginalName::Unverified;
                let staged = if receipt.cleanup != Cleanup::Complete {
                    reviewed_stage_path(&released.native)
                } else {
                    None
                };
                if !confirmed {
                    row.not_removed.push(reviewed_not_removed(
                        mark,
                        name,
                        REVIEW_UNCERTAIN,
                        staged,
                    ));
                }
                let index = execution.attempts.len();
                execution.attempts.push(ReviewedFileRemoval {
                    relative: reviewed_relative(mark, name),
                    class: mark.class,
                    native: released.native,
                    #[cfg(unix)]
                    append: None,
                    #[cfg(unix)]
                    marker: None,
                    binding: None,
                    settled: false,
                });
                attempted.insert((group_index, name.clone()), index);
                checkpoint(
                    ReviewedCheckpoint::AfterRaw,
                    &group.directory.diagnostic_path().join(name),
                );
                if unresolved {
                    stop = true;
                }
            }
            execution.outcome.targets.push(row);
        }
    }
    checkpoint(ReviewedCheckpoint::BeforeBookkeeping, root.canonical_path());
    for ((group_index, name), index) in &attempted {
        let group = groups
            .get(*group_index)
            .ok_or(ReviewedRemovalRefusal::Changed)?;
        let attempt = execution
            .attempts
            .get_mut(*index)
            .ok_or(ReviewedRemovalRefusal::Changed)?;
        let native = attempt.native.receipt();
        if native.effect == RemovalEffect::Deleted
            && native.original_name == OriginalName::Absent
            && native.claim_confirmation == Confirmation::Confirmed
        {
            reviewed_bookkeeping(&root, group, name, now, attempt);
        }
    }
    // All overlapping rows share the single physical operation's settlement facts.
    let mut plans = Vec::new();
    for (mark, row) in checked.marks().iter().zip(&mut execution.outcome.targets) {
        let group_index = groups
            .iter()
            .position(|g| g.target == mark.target && g.layout == reviewed_layout(mark))
            .ok_or(ReviewedRemovalRefusal::Changed)?;
        let group = groups
            .get(group_index)
            .ok_or(ReviewedRemovalRefusal::Changed)?;
        let mut complete = true;
        let mut failed = false;
        let mut staged = None;
        let mut remaining = Vec::new();
        for name in &mark.proposal.names {
            if let Some(index) = attempted.get(&(group_index, name.clone())) {
                let attempt = execution
                    .attempts
                    .get(*index)
                    .ok_or(ReviewedRemovalRefusal::Changed)?;
                let receipt = attempt.native.receipt();
                if staged.is_none() && receipt.cleanup != Cleanup::Complete {
                    staged = reviewed_stage_path(&attempt.native);
                }
                if receipt.cleanup != Cleanup::Complete {
                    complete = false;
                    if !remaining.contains(name) {
                        remaining.push(name.clone());
                    }
                    failed = true;
                }
                if !attempt.settled {
                    complete = false;
                    if !remaining.contains(name) {
                        remaining.push(name.clone());
                    }
                    if receipt.effect != RemovalEffect::NotClaimed
                        || receipt.cleanup != Cleanup::Complete
                    {
                        failed = true;
                    }
                    if receipt.effect == RemovalEffect::Deleted {
                        row.post_commit_failure = Some(crate::receipt::PostCommitFailure {
                            entry: attempt.relative.clone(),
                            reason: REVIEW_POST_COMMIT.to_owned(),
                        });
                    } else if !row
                        .not_removed
                        .iter()
                        .any(|item| item.entry == attempt.relative)
                    {
                        row.not_removed.push(reviewed_not_removed(
                            mark,
                            name,
                            REVIEW_UNCERTAIN,
                            reviewed_stage_path(&attempt.native),
                        ));
                    }
                }
            } else if let Some(index) = stage_failed.get(&(group_index, name.clone())) {
                complete = false;
                remaining.push(name.clone());
                failed = true;
                staged = execution
                    .stage_failures
                    .get(*index)
                    .and_then(|f| f.owned.as_ref())
                    .map(|s| {
                        journal_rel(
                            s.diagnostic_path()
                                .strip_prefix(root.canonical_path())
                                .unwrap_or_else(|_| Path::new("")),
                        )
                    });
            } else if group.scan.files().iter().any(|f| f.name.as_str() == name) {
                complete = false;
                remaining.push(name.clone());
            }
        }
        let plan = if failed {
            Some(PlannedMarkMutation::failure(
                mark,
                Failure {
                    at: now.to_rfc3339_opts(SecondsFormat::Secs, true),
                    reason: REVIEW_UNCERTAIN.to_owned(),
                    staged,
                },
            ))
        } else if complete {
            Some(PlannedMarkMutation::resolve(mark))
        } else if mark.offload.is_some() && remaining != mark.proposal.names {
            Some(PlannedMarkMutation::remainder(mark, &remaining))
        } else {
            None
        };
        if let Some(plan) = plan {
            plans.push(plan.map_err(|e| {
                ReviewedRemovalRefusal::Mutation(
                    crate::mark_mutation::BoundMutationRefusal::Review(
                        crate::review_context::ReviewRefusal::Register(e),
                    ),
                )
            })?);
        }
    }
    checkpoint(ReviewedCheckpoint::BeforeFinalCas, root.canonical_path());
    let final_check = (|| {
        policy.revalidate()?;
        reviewed_admission(archive, execution)?;
        Ok::<(), ReviewedRemovalRefusal>(())
    })();
    if let Err(error) = final_check {
        reviewed_mark_unsaved_deletions(&mut execution.outcome);
        return Err(error);
    }
    execution.mutation = Some(acquire_register().and_then(|guard| guard.publish(&plans)));
    if !matches!(
        execution.mutation.as_ref(),
        Some(Ok(crate::mark_mutation::BoundMutationOutcome::Published(_)
            | crate::mark_mutation::BoundMutationOutcome::Unchanged(_)))
    ) {
        reviewed_mark_unsaved_deletions(&mut execution.outcome);
    }
    if stop {
        execution.outcome.halted = Some(RunHalt {
            reason: REVIEW_HALTED.to_owned(),
        });
    }
    Ok(())
}

#[cfg(any(unix, windows))]
fn reviewed_not_removed(
    mark: &Mark,
    name: &str,
    reason: &str,
    staged: Option<String>,
) -> NotRemoved {
    NotRemoved {
        entry: reviewed_relative(mark, name),
        reason: reason.to_owned(),
        staged,
    }
}

#[cfg(any(unix, windows))]
fn reviewed_stage_path(removal: &solstone_core_journal_io::media::MediaRemoval) -> Option<String> {
    let stage = removal.stage_observation()?;
    let mut parts = stage.parent_relative.clone();
    parts.push(stage.directory_name.clone());
    Some(parts.join("/"))
}

// A staged path is a journal coordinate, `/`-separated on every platform as
// the rest of the retention door writes it; Windows would otherwise spell it
// with backslashes.
#[cfg(any(unix, windows))]
fn journal_rel(path: &Path) -> String {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(unix)]
fn reviewed_bookkeeping(
    root: &std::sync::Arc<solstone_core_journal_io::JournalRoot>,
    group: &ReviewedGroup,
    name: &str,
    now: DateTime<Utc>,
    attempt: &mut ReviewedFileRemoval,
) {
    use solstone_core_journal_io::media::Durability;
    use std::ffi::OsStr;
    let bindings = || -> std::io::Result<()> {
        root.revalidate_canonical_binding()
            .map_err(std::io::Error::other)?;
        group.directory.revalidate()?;
        group.events.revalidate().map_err(std::io::Error::other)?;
        group.health.revalidate().map_err(std::io::Error::other)?;
        let events =
            solstone_core_journal_io::FlatDirectory::open(root, Path::new(&group.relative()))
                .map_err(std::io::Error::other)?;
        let health = solstone_core_journal_io::FlatDirectory::open(
            root,
            Path::new(&crate::layout::day_health_rel(&group.target.day)),
        )
        .map_err(std::io::Error::other)?;
        if events.identity() != group.events.identity()
            || health.identity() != group.health.identity()
        {
            return Err(std::io::Error::other("removal bookkeeping binding changed"));
        }
        Ok(())
    };
    if let Err(error) = bindings() {
        attempt.binding = Some(error);
        return;
    }
    let class = match attempt.class {
        RemovalClass::PolicyRawRelease => RawReleaseClass::Policy,
        _ => RawReleaseClass::Offload,
    };
    let mut extra = serde_json::Map::new();
    extra.insert(
        "name".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    extra.insert(
        "class".to_owned(),
        serde_json::Value::String(class.tag().to_owned()),
    );
    let event = solstone_core_callosum::CallosumEnvelope {
        tract: "retention".to_owned(),
        event: "original_deleted".to_owned(),
        ts: Some(now.timestamp_millis()),
        extra,
    };
    attempt.append = Some(solstone_core_journal_io::append::append_jsonl_bound(
        &group.events,
        OsStr::new("events.jsonl"),
        &event,
    ));
    attempt.marker = Some(
        solstone_core_journal_io::health_marker::bump_stream_marker_bound(
            &group.health,
            LockOptions {
                timeout: std::time::Duration::ZERO,
                ..LockOptions::default()
            },
        ),
    );
    attempt.binding = bindings().err();
    let native = attempt.native.receipt();
    let native_settled = native.source_durability == Durability::Synced
        && native.claim_durability == Durability::Synced;
    let append_settled = attempt.append.as_ref().is_some_and(|a| {
        a.confirmation() == solstone_core_journal_io::append::BoundAppendConfirmation::Confirmed
            && a.file_durability()
                == solstone_core_journal_io::append::BoundAppendDurability::Synced
            && a.parent_durability()
                == solstone_core_journal_io::append::BoundAppendDurability::Synced
            && a.failures().is_empty()
    });
    let marker_settled = attempt.marker.as_ref().is_some_and(|m| match m {
        Ok(m) => {
            matches!(
                m.publication(),
                solstone_core_journal_io::BoundAtomicOutcome::Published { .. }
            ) && m.binding_error().is_none()
                && m.lock_error().is_none()
        }
        Err(_) => false,
    });
    attempt.settled =
        native_settled && append_settled && marker_settled && attempt.binding.is_none();
}

#[cfg(windows)]
fn append_windows_removal_event(
    parent: &solstone_core_journal_io::WindowsFlatDirectory,
    diagnostic: &Path,
    event: &solstone_core_callosum::CallosumEnvelope,
) -> std::io::Result<()> {
    use std::io::{Read, Seek, SeekFrom, Write};

    let mut contents = serde_json::to_vec(event).map_err(std::io::Error::other)?;
    contents.push(b'\n');
    parent.revalidate_bound().map_err(std::io::Error::other)?;
    let mut file =
        solstone_core_journal_io::windows_sync_dir::create_or_open_windows_append_file_bound(
            parent,
            std::ffi::OsStr::new("events.jsonl"),
            diagnostic,
        )
        .map_err(std::io::Error::other)?;
    if file.seek(SeekFrom::End(0))? != 0 {
        file.seek(SeekFrom::End(-1))?;
        let mut tail = [0];
        file.read_exact(&mut tail)?;
        if tail != *b"\n" {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "removal event log has an unfinished record",
            ));
        }
    }
    parent.revalidate_bound().map_err(std::io::Error::other)?;
    file.write_all(&contents)?;
    file.sync_all()?;
    parent.revalidate_bound().map_err(std::io::Error::other)
}

#[cfg(windows)]
fn reviewed_bookkeeping(
    root: &std::sync::Arc<solstone_core_journal_io::JournalRoot>,
    group: &ReviewedGroup,
    name: &str,
    now: DateTime<Utc>,
    attempt: &mut ReviewedFileRemoval,
) {
    use solstone_core_journal_io::media::{
        Confirmation, Durability, OriginalName, RemovalEffect, RemovalPhase,
    };
    use solstone_core_journal_io::{
        DetailedAtomicOutcome, HealthMarker, acquire_existing_parent_lock_bound,
        atomic_replace_detailed_bound, read_windows_observed_file_bounded,
    };
    use std::ffi::OsStr;
    use std::time::Duration;

    let bindings = || -> std::io::Result<()> {
        root.revalidate_canonical_binding()
            .map_err(std::io::Error::other)?;
        group.directory.revalidate()?;
        group
            .events
            .revalidate_bound()
            .map_err(std::io::Error::other)?;
        group
            .health
            .revalidate_bound()
            .map_err(std::io::Error::other)?;
        let events = open_windows_directory_path(root, Path::new(&group.relative()))
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        let health = open_windows_directory_path(
            root,
            Path::new(&crate::layout::day_health_rel(&group.target.day)),
        )
        .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        if events.object_identity() != group.events.object_identity()
            || health.object_identity() != group.health.object_identity()
        {
            return Err(std::io::Error::other("removal bookkeeping binding changed"));
        }
        Ok(())
    };

    if let Err(error) = bindings() {
        attempt.binding = Some(error);
        return;
    }

    let class = match attempt.class {
        RemovalClass::PolicyRawRelease => RawReleaseClass::Policy,
        _ => RawReleaseClass::Offload,
    };
    let mut extra = serde_json::Map::new();
    extra.insert(
        "name".to_owned(),
        serde_json::Value::String(name.to_owned()),
    );
    extra.insert(
        "class".to_owned(),
        serde_json::Value::String(class.tag().to_owned()),
    );
    let event = solstone_core_callosum::CallosumEnvelope {
        tract: "retention".to_owned(),
        event: "original_deleted".to_owned(),
        ts: Some(now.timestamp_millis()),
        extra,
    };

    let append_result = append_windows_removal_event(
        &group.events,
        &root.canonical_path().join(group.relative()),
        &event,
    );

    let bump_marker = || -> Result<bool, std::io::Error> {
        let lock_name = OsStr::new("stream.updated.lock");
        let lock = acquire_existing_parent_lock_bound(
            &group.health,
            lock_name,
            Duration::ZERO,
            Duration::ZERO,
        )
        .map_err(|e| std::io::Error::other(format!("{e:?}")))?;
        group
            .health
            .revalidate_bound()
            .map_err(std::io::Error::other)?;
        lock.revalidate_bound(&group.health, lock_name)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;

        let observed = read_windows_observed_file_bounded(
            &group.health,
            OsStr::new("stream.updated"),
            1024 * 1024,
        )
        .map_err(std::io::Error::other)?;
        let (generation, fingerprint) = match observed {
            None => (0, None),
            Some(observed) if observed.bytes.is_empty() => (0, None),
            Some(observed) => {
                let marker = serde_json::from_slice::<HealthMarker>(&observed.bytes)
                    .map_err(std::io::Error::other)?;
                if marker.version != 1 {
                    return Err(std::io::Error::other("unexpected marker version"));
                }
                (marker.generation, marker.fingerprint)
            }
        };
        let generation = generation
            .checked_add(1)
            .ok_or_else(|| std::io::Error::other("marker generation overflow"))?;
        let marker = HealthMarker {
            version: 1,
            generation,
            fingerprint,
        };
        let contents = serde_json::to_vec(&marker).map_err(std::io::Error::other)?;
        group
            .health
            .revalidate_bound()
            .map_err(std::io::Error::other)?;
        lock.revalidate_bound(&group.health, lock_name)
            .map_err(|e| std::io::Error::other(format!("{e:?}")))?;

        let publication = atomic_replace_detailed_bound(
            &group.health,
            &lock,
            OsStr::new("stream.updated"),
            &contents,
            0o600,
        );
        let confirmed = matches!(publication, Ok(DetailedAtomicOutcome::Published));
        let bound = group.health.revalidate_bound().is_ok()
            && lock.revalidate_bound(&group.health, lock_name).is_ok();
        Ok(confirmed && bound)
    };

    let marker_published = bump_marker().unwrap_or(false);

    attempt.binding = bindings().err();
    let native = attempt.native.receipt();
    attempt.settled = native.effect == RemovalEffect::Deleted
        && native.original_name == OriginalName::Absent
        && native.claim_confirmation == Confirmation::Confirmed
        && (native.source_durability == Durability::Synced
            || native.source_durability == Durability::Unproven)
        && (native.claim_durability == Durability::Synced
            || native.claim_durability == Durability::Unproven)
        && native
            .failures
            .iter()
            .all(|failure| failure.phase == RemovalPhase::Cleanup)
        && append_result.is_ok()
        && marker_published
        && attempt.binding.is_none();
}

#[cfg(all(test, unix, feature = "full-tests"))]
#[path = "../tests/support/reviewed_removal.rs"]
mod reviewed_tests;

#[cfg(test)]
#[allow(
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic,
    clippy::unwrap_used,
    reason = "test setup and assertions use concise infallible helpers"
)]
mod tests {
    use std::fs;
    use std::path::Path;

    use chrono::TimeZone;

    use super::*;
    use crate::marks::{Proposal, RemovalClass, decline, preflight, reconcile};
    use crate::policy::policy_from_retention;
    use serde_json::json;

    #[test]
    #[cfg(any(unix, windows))]
    fn staged_paths_are_slash_separated_on_every_platform() {
        let native = Path::new("20260805")
            .join("field.audio")
            .join(".070000_17.removing");
        assert_eq!(
            journal_rel(&native),
            "20260805/field.audio/.070000_17.removing"
        );
    }

    #[test]
    #[cfg(windows)]
    fn removal_events_preserve_linked_and_unfinished_files() {
        use solstone_core_journal_io::{JournalRoot, open_windows_flat_directory_bound};
        use std::{ffi::OsStr, sync::Arc};

        let event = solstone_core_callosum::CallosumEnvelope {
            tract: "retention".to_owned(),
            event: "original_deleted".to_owned(),
            ts: Some(1),
            extra: serde_json::Map::new(),
        };
        for (initial, linked, allowed) in [
            (b"".as_slice(), false, true),
            (b"{}\n".as_slice(), false, true),
            (b"{unfinished".as_slice(), false, false),
            (b"owner's other file\n".as_slice(), true, false),
        ] {
            let journal = tempfile::tempdir().unwrap();
            let directory = journal.path().join("segment");
            fs::create_dir(&directory).unwrap();
            let path = directory.join("events.jsonl");
            let other = journal.path().join("other");
            fs::write(&path, initial).unwrap();
            if linked {
                fs::hard_link(&path, &other).unwrap();
            }
            let root = Arc::new(JournalRoot::open(journal.path()).unwrap());
            let parent = open_windows_flat_directory_bound(
                root.as_ref(),
                OsStr::new("segment"),
                root.canonical_path(),
            )
            .unwrap()
            .unwrap();
            let result = append_windows_removal_event(&parent, &directory, &event);
            assert_eq!(
                result.is_ok(),
                allowed,
                "linked={linked} initial={initial:?}"
            );
            let bytes = fs::read(&path).unwrap();
            if allowed {
                assert!(bytes.starts_with(initial));
                let appended: solstone_core_callosum::CallosumEnvelope =
                    serde_json::from_slice(&bytes[initial.len()..]).unwrap();
                assert_eq!(appended.event, event.event);
            } else {
                assert_eq!(bytes, initial);
            }
            if linked {
                assert_eq!(fs::read(other).unwrap(), initial);
            }
        }
    }

    #[test]
    fn a_mark_declined_after_preflight_is_not_unlinked() {
        let journal = tempfile::tempdir().unwrap();
        let target = crate::Target {
            day: "20260701".to_owned(),
            stream: "field.audio".to_owned(),
            dir: "070000_17".to_owned(),
        };
        let segment = journal.path().join(crate::layout::segment_rel(
            &target.day,
            &target.stream,
            &target.dir,
        ));
        fs::create_dir_all(&segment).unwrap();
        let raw = segment.join("audio.flac");
        fs::write(&raw, b"the owner's originals").unwrap();
        let proposal = Proposal {
            bytes: 1,
            reason: "test approval".to_owned(),
            names: vec!["audio.flac".to_owned()],
        };
        let register = reconcile(
            journal.path(),
            RemovalClass::PolicyRawRelease,
            &[(target.clone(), proposal.clone())],
            crate::marks::mark_at("first"),
        )
        .unwrap();
        let id = register.marks.keys().next().unwrap().clone();
        let marks = preflight(journal.path(), std::slice::from_ref(&id)).unwrap();
        decline(journal.path(), &id).unwrap();
        let policy = Policy {
            default_rule: crate::Rule {
                anchor: crate::Anchor::Captured,
                period: Some(crate::Days(1)),
                priority: 0,
            },
            enabled: true,
            ..Policy::default()
        };
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap();
        let mut register_errors = Vec::new();
        let outcome = remove_marked(
            journal.path(),
            &marks,
            &policy,
            today,
            now,
            &mut register_errors,
        );

        assert!(raw.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(outcome.targets[0].removed, Vec::new());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert_eq!(
            outcome.targets[0].not_removed[0].reason,
            NO_LONGER_ON_REMOVAL_LIST
        );
    }

    fn target() -> crate::Target {
        crate::Target {
            day: "20260701".to_owned(),
            stream: "field.audio".to_owned(),
            dir: "070000_17".to_owned(),
        }
    }

    fn keep_journal_product_policy() -> Policy {
        policy_from_retention(json!({"raw_media": "keep"}).as_object().unwrap())
    }

    fn seed_empty_audio(journal: &Path, target: &crate::Target) -> std::path::PathBuf {
        let segment = journal.join(crate::layout::segment_rel(
            &target.day,
            &target.stream,
            &target.dir,
        ));
        fs::create_dir_all(&segment).unwrap();
        let raw = b"raw";
        fs::write(segment.join("audio.flac"), raw).unwrap();
        let header = json!({
            "segment": &target.dir,
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "empty",
                "reason_code": "no_decodable_audio",
                "handler": "transcribe",
                "attempted_at": "2026-07-01T00:00:00Z",
                "input_size": raw.len(),
            }
        });
        fs::write(segment.join("audio.jsonl"), format!("{header}\n")).unwrap();
        segment
    }

    fn seed_analyzed_sibling(segment: &Path, name: &str) {
        let raw = b"sibling";
        fs::write(segment.join(name), raw).unwrap();
        let stem = name.rsplit_once('.').unwrap().0;
        let header = json!({
            "segment": "070000_17",
            "_solstone_processing": {
                "schema": "solstone.processing.v1",
                "state": "analyzed",
                "reason_code": "ok",
                "handler": "transcribe",
                "attempted_at": "2026-07-01T00:00:00Z",
                "input_size": raw.len(),
            }
        });
        fs::write(
            segment.join(format!("{stem}.jsonl")),
            format!("{header}\n{{\"start\":0.0,\"text\":\"x\"}}\n"),
        )
        .unwrap();
    }

    fn approve(
        journal: &Path,
        names: Vec<String>,
        policy: &Policy,
    ) -> (crate::Outcome, Vec<String>) {
        let target = target();
        let proposal = Proposal {
            bytes: 1,
            reason: "test approval".to_owned(),
            names,
        };
        let register = reconcile(
            journal,
            RemovalClass::PolicyRawRelease,
            &[(target, proposal)],
            crate::marks::mark_at("first"),
        )
        .unwrap();
        let id = register.marks.keys().next().unwrap().clone();
        let marks = preflight(journal, std::slice::from_ref(&id)).unwrap();
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap();
        let mut register_errors = Vec::new();
        let outcome = remove_marked(journal, &marks, policy, today, now, &mut register_errors);
        (outcome, register_errors)
    }

    #[test]
    fn an_unnamed_ordinary_sibling_does_not_block_or_appear_on_an_empty_audio_mark() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        seed_analyzed_sibling(&segment, "extra.flac");
        let audio = segment.join("audio.flac");
        let extra = segment.join("extra.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) =
            approve(journal.path(), vec!["audio.flac".to_owned()], &policy);

        assert!(!audio.exists());
        assert!(extra.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets.len(), 1);
        assert_eq!(outcome.targets[0].removed.len(), 1);
        assert!(
            outcome.targets[0].removed[0]
                .as_str()
                .ends_with("audio.flac")
        );
        assert_eq!(outcome.targets[0].not_removed, Vec::new());
    }

    #[test]
    fn a_mark_naming_both_sides_releases_only_the_eligible_file() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        seed_analyzed_sibling(&segment, "extra.flac");
        let audio = segment.join("audio.flac");
        let extra = segment.join("extra.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) = approve(
            journal.path(),
            vec!["audio.flac".to_owned(), "extra.flac".to_owned()],
            &policy,
        );

        assert!(!audio.exists());
        assert!(extra.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets[0].removed.len(), 1);
        assert!(
            outcome.targets[0].removed[0]
                .as_str()
                .ends_with("audio.flac")
        );
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert!(
            outcome.targets[0].not_removed[0]
                .entry
                .ends_with("extra.flac")
        );
        assert_eq!(outcome.targets[0].not_removed[0].reason, KEPT_FOREVER);
    }

    #[test]
    fn a_named_file_that_is_no_longer_empty_terminal_is_refused_per_file() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        let audio = segment.join("audio.flac");
        seed_analyzed_sibling(&segment, "audio.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) =
            approve(journal.path(), vec!["audio.flac".to_owned()], &policy);

        assert!(audio.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets[0].removed, Vec::new());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert!(
            outcome.targets[0].not_removed[0]
                .entry
                .ends_with("audio.flac")
        );
        assert_eq!(outcome.targets[0].not_removed[0].reason, KEPT_FOREVER);
    }

    #[test]
    fn ac3_a_policy_approval_writes_policy_record_in_events_jsonl() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        let audio = segment.join("audio.flac");
        let policy = keep_journal_product_policy();
        let (outcome, register_errors) =
            approve(journal.path(), vec!["audio.flac".to_owned()], &policy);

        assert!(!audio.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets[0].removed.len(), 1);

        let events_path = segment.join("events.jsonl");
        assert!(events_path.is_file());
        let lines: Vec<String> = fs::read_to_string(&events_path)
            .unwrap()
            .lines()
            .map(str::to_owned)
            .collect();
        assert_eq!(lines.len(), 1);
        let row: serde_json::Value = serde_json::from_str(&lines[0]).unwrap();
        assert_eq!(row["tract"], "retention");
        assert_eq!(row["event"], "original_deleted");
        assert_eq!(row["name"], "audio.flac");
        assert_eq!(row["class"], "policy_raw_release");
    }

    #[test]
    fn ac3_b_unbound_offload_marks_never_fall_through_to_the_policy() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap();
        let register = crate::marks::upsert_offload(
            journal.path(),
            &target,
            vec!["audio.flac".to_owned()],
            100,
            "backup offloaded".to_owned(),
            now,
        )
        .unwrap();

        let id = MarkId::derive(
            RemovalClass::OffloadRawRelease,
            &target,
            &["audio.flac".to_owned()],
        );
        let mark = register.marks.get(&id).unwrap().clone();
        let preflight = PreflightMarks::new_for_test(vec![(id, mark)]);
        let policy = keep_journal_product_policy();
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let mut register_errors = Vec::new();
        let outcome = remove_marked(
            journal.path(),
            &preflight,
            &policy,
            today,
            now,
            &mut register_errors,
        );

        assert_eq!(outcome.targets.len(), 1);
        assert!(outcome.targets[0].removed.is_empty());
        assert_eq!(outcome.targets[0].not_removed[0].reason, UNBOUND_OFFLOAD);
        assert!(segment.join("audio.flac").exists());
        assert!(!segment.join("events.jsonl").exists());
    }

    #[test]
    fn ac3_e_non_raw_release_mark_is_refused_with_exact_copy_and_does_not_unlink() {
        let journal = tempfile::tempdir().unwrap();
        let target = target();
        let segment = seed_empty_audio(journal.path(), &target);
        let audio = segment.join("audio.flac");
        let proposal = Proposal {
            bytes: 1,
            reason: "non raw mark".to_owned(),
            names: vec!["audio.flac".to_owned()],
        };
        let id = MarkId::derive(RemovalClass::OwnerRawRelease, &target, &proposal.names);
        let mark = Mark {
            id: id.clone(),
            class: RemovalClass::OwnerRawRelease,
            target: target.clone(),
            marked_at: "2026-08-06T00:00:00Z".to_owned(),
            proposal: proposal.clone(),
            state: crate::marks::MarkState::Marked,
            offload: None,
        };
        let mut reg = crate::marks::Register::empty();
        reg.marks.insert(id.clone(), mark.clone());
        let reg_path = journal.path().join("health/retention-marks.json");
        fs::create_dir_all(reg_path.parent().unwrap()).unwrap();
        fs::write(&reg_path, serde_json::to_string(&reg).unwrap()).unwrap();

        let preflight = PreflightMarks::new_for_test(vec![(id, mark)]);
        let policy = keep_journal_product_policy();
        let today = NaiveDate::from_ymd_opt(2026, 8, 6).unwrap();
        let now = Utc.with_ymd_and_hms(2026, 8, 6, 0, 0, 0).single().unwrap();
        let mut register_errors = Vec::new();
        let outcome = remove_marked(
            journal.path(),
            &preflight,
            &policy,
            today,
            now,
            &mut register_errors,
        );

        assert!(audio.exists());
        assert!(register_errors.is_empty());
        assert_eq!(outcome.targets[0].removed, Vec::new());
        assert_eq!(outcome.targets[0].not_removed.len(), 1);
        assert_eq!(
            outcome.targets[0].not_removed[0].reason,
            "this mark is not an original-media deletion that can be approved from the removal list"
        );
        assert!(!segment.join("events.jsonl").exists());
    }
}
