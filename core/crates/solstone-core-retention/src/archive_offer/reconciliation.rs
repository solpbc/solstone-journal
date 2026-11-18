// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Withdraw currently kept empty-audio offers and legacy unbound offload marks
//! through the retained register.

use std::sync::Arc;

use jiff::Timestamp;
use jiff::civil::Date;
use solstone_core_backup::archive_activity::PreparedArchiveOperation;
use solstone_core_journal_io::JournalRoot;

use super::{OffloadOfferRefusal, OffloadOfferSelection};
use crate::mark_mutation::{BoundMutationOutcome, BoundMutationRefusal};
use crate::marks::{Mark, Register};

#[derive(Debug)]
pub enum OffloadReconciliationDisposition {
    Unchanged,
    Busy,
    Refused(Box<OffloadOfferRefusal>),
    Publication {
        outcome: Box<BoundMutationOutcome>,
        observation: Option<Box<OffloadOfferRefusal>>,
    },
    WriteRefused(Box<BoundMutationRefusal>),
}

pub struct OffloadReconciliationGroup {
    selection: Option<OffloadOfferSelection>,
    expected: Vec<Mark>,
    disposition: OffloadReconciliationDisposition,
}

impl OffloadReconciliationGroup {
    pub fn selection(&self) -> Option<&OffloadOfferSelection> {
        self.selection.as_ref()
    }
    pub fn expected(&self) -> &[Mark] {
        &self.expected
    }
    pub fn disposition(&self) -> &OffloadReconciliationDisposition {
        &self.disposition
    }
}

/// Metadata observations only. This report supplies no pending-byte credit.
#[must_use]
pub struct OffloadReconciliation {
    root: Arc<JournalRoot>,
    register: Register,
    groups: Vec<OffloadReconciliationGroup>,
}

impl OffloadReconciliation {
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn register(&self) -> &Register {
        &self.register
    }
    pub fn groups(&self) -> &[OffloadReconciliationGroup] {
        &self.groups
    }

    /// Consume the actual reconciliation facts without retaining its root handle.
    pub fn into_report(self) -> OffloadReconciliationReport {
        OffloadReconciliationReport {
            root_observation: self.root.identity().observation_id(),
            register: self.register,
            groups: self.groups,
        }
    }
}

/// Actual reconciliation data; this report grants no mutation or finish authority.
pub struct OffloadReconciliationReport {
    root_observation: String,
    register: Register,
    groups: Vec<OffloadReconciliationGroup>,
}

impl OffloadReconciliationReport {
    pub fn root_observation(&self) -> &str {
        &self.root_observation
    }
    pub fn register(&self) -> &Register {
        &self.register
    }
    pub fn groups(&self) -> &[OffloadReconciliationGroup] {
        &self.groups
    }
}

#[derive(Debug)]
pub enum OffloadReconciliationStop {
    Observation(Box<OffloadOfferRefusal>),
    Publication,
}

/// Preserve earlier actual effects when a later observation or write fails.
#[must_use]
pub struct OffloadReconciliationFailure {
    root: Arc<JournalRoot>,
    last_confirmed: Option<Register>,
    groups: Vec<OffloadReconciliationGroup>,
    reason: OffloadReconciliationStop,
}

impl OffloadReconciliationFailure {
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        &self.root
    }
    pub fn last_confirmed_register(&self) -> Option<&Register> {
        self.last_confirmed.as_ref()
    }
    pub fn groups(&self) -> &[OffloadReconciliationGroup] {
        &self.groups
    }
    pub fn reason(&self) -> &OffloadReconciliationStop {
        &self.reason
    }
}

/// Reconcile before considering the budget, using the original preparation.
/// Only current kept empty audio and legacy unbound offload marks are
/// withdrawn; no archive or raw file changes.
pub fn reconcile_offload_empty(
    operation: &PreparedArchiveOperation<'_>,
    today: Date,
    now: Timestamp,
) -> Result<OffloadReconciliation, OffloadReconciliationFailure> {
    #[cfg(any(unix, windows))]
    {
        unix::reconcile(operation, today, now, &mut unix::Hooks::default())
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _parameters = (today, now);
        Err(OffloadReconciliationFailure {
            root: Arc::clone(operation.journal_root()),
            last_confirmed: None,
            groups: Vec::new(),
            reason: OffloadReconciliationStop::Observation(Box::new(
                OffloadOfferRefusal::Unsupported,
            )),
        })
    }
}

#[cfg(any(unix, windows))]
pub(crate) mod unix {
    use super::*;
    use std::collections::{BTreeMap, BTreeSet};
    use std::ffi::OsStr;
    use std::io;
    use std::path::Path;

    use solstone_core_journal_io::media::MediaDirectory;
    use solstone_core_segment::output_attempt::{OutputClaims, SegmentCommit};
    use solstone_core_segment::{OwnerDeletionState, RetainedOwnerDeletion};

    use crate::content::{ClosedHandlerSet, JournalMedia};
    use crate::eligibility::{RawRelease, resolve};
    use crate::grouped::{DecisionPurpose, GroupVerdict, inspect_grouped};
    use crate::mark_mutation::PlannedMarkMutation;
    use crate::marks::{MarkState, RemovalClass};
    use crate::remove_marked::ReviewedPolicy;
    use crate::scan::CheckedSegment;

    #[derive(Default)]
    pub(super) struct Hooks {
        #[cfg(all(test, feature = "full-tests"))]
        before_publish: Option<Box<dyn FnMut()>>,
    }

    impl Hooks {
        fn before_publish(&mut self) {
            #[cfg(all(test, feature = "full-tests"))]
            if let Some(hook) = &mut self.before_publish {
                hook();
            }
        }
    }

    /// Only the retained canonical observation below constructs this value.
    pub(crate) struct ReconciliationObservation<'a, 'clock> {
        operation: &'a PreparedArchiveOperation<'clock>,
        directory: &'a MediaDirectory,
        owner: &'a RetainedOwnerDeletion,
        scan: &'a CheckedSegment,
        claims: &'a OutputClaims,
        commit: &'a SegmentCommit<'a>,
        policy: &'a ReviewedPolicy,
        expected: &'a [Mark],
        plans: Vec<PlannedMarkMutation>,
    }

    impl ReconciliationObservation<'_, '_> {
        pub(crate) fn journal_root(&self) -> &JournalRoot {
            self.operation.journal_root()
        }
        pub(crate) fn expected(&self) -> &[Mark] {
            self.expected
        }
        pub(crate) fn plans(&self) -> &[PlannedMarkMutation] {
            &self.plans
        }
        pub(crate) fn revalidate(&self) -> Result<(), OffloadOfferRefusal> {
            self.operation
                .revalidate()
                .map_err(OffloadOfferRefusal::Admission)?;
            self.policy
                .revalidate()
                .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
            self.directory.revalidate()?;
            if self.owner.state().map_err(io::Error::other)? != OwnerDeletionState::Live {
                return Err(OffloadOfferRefusal::Changed);
            }
            self.commit.revalidate()?;
            let absolute = self.directory.diagnostic_path();
            self.commit.require_outputs(
                self.claims,
                absolute,
                self.expected
                    .iter()
                    .flat_map(|mark| &mark.proposal.names)
                    .map(|name| absolute.join(name).with_extension("jsonl")),
            )?;
            let current = super::super::scan(self.directory)?;
            if current.files().len() != self.scan.files().len()
                || self
                    .scan
                    .files()
                    .iter()
                    .zip(current.files())
                    .any(|(before, after)| {
                        before.name != after.name
                            || before.size != after.size
                            || before.sidecar.record != after.sidecar.record
                            || before.sidecar.has_analysis_row != after.sidecar.has_analysis_row
                            || self.scan.bound_entry(&before.name)
                                != current.bound_entry(&after.name)
                    })
            {
                return Err(OffloadOfferRefusal::Changed);
            }
            self.operation
                .revalidate()
                .map_err(OffloadOfferRefusal::Admission)
        }
    }

    fn binding(
        root: &Arc<JournalRoot>,
        mark: &Mark,
    ) -> Result<OffloadOfferSelection, OffloadOfferRefusal> {
        let offer = mark.offload.as_ref().ok_or(OffloadOfferRefusal::Changed)?;
        if offer.proof.journal_id != root.identity().observation_id() {
            return Err(OffloadOfferRefusal::Changed);
        }
        Ok(OffloadOfferSelection {
            target: mark.target.clone(),
            layout: offer.proof.layout,
            names: mark.proposal.names.clone(),
        })
    }

    /// An offload mark without an archive offer has no backup proof and can
    /// never be approved, while it keeps the register from holding any offer.
    /// Withdrawing it decides nothing about its files: only the exact observed
    /// row is removed, under the register lock, so the planner can offer those
    /// files again with proof.
    fn withdraw_unbound(
        operation: &PreparedArchiveOperation<'_>,
        mark: &Mark,
    ) -> OffloadReconciliationDisposition {
        if let Err(error) = operation.revalidate() {
            return OffloadReconciliationDisposition::Refused(Box::new(
                OffloadOfferRefusal::Admission(error),
            ));
        }
        let plan = match PlannedMarkMutation::resolve(mark) {
            Ok(plan) => plan,
            Err(error) => {
                return OffloadReconciliationDisposition::Refused(Box::new(
                    OffloadOfferRefusal::Review(crate::review_context::ReviewRefusal::Register(
                        error,
                    )),
                ));
            }
        };
        match crate::mark_mutation::conditional_mutate_at_root(operation.journal_root(), &plan) {
            Ok(outcome) => OffloadReconciliationDisposition::Publication {
                outcome: Box::new(outcome),
                observation: None,
            },
            Err(error) => OffloadReconciliationDisposition::WriteRefused(Box::new(error)),
        }
    }

    /// Record one group; true when the run must stop after its write.
    fn settle(
        failure: &mut OffloadReconciliationFailure,
        group: OffloadReconciliationGroup,
    ) -> bool {
        let disposition = &group.disposition;
        let stop = matches!(
            disposition,
            OffloadReconciliationDisposition::WriteRefused(error)
                if matches!(error.as_ref(), BoundMutationRefusal::Write(_))
        ) || matches!(disposition, OffloadReconciliationDisposition::Publication { outcome, .. }
                if matches!(outcome.as_ref(), BoundMutationOutcome::PublishedUncertain { .. }))
            || matches!(
                disposition,
                OffloadReconciliationDisposition::Publication {
                    observation: Some(_),
                    ..
                }
            );
        if let OffloadReconciliationDisposition::Publication {
            outcome,
            observation: None,
        } = disposition
        {
            match outcome.as_ref() {
                BoundMutationOutcome::Published(result)
                | BoundMutationOutcome::Unchanged(result) => {
                    failure.last_confirmed = Some(result.register.clone())
                }
                BoundMutationOutcome::PublishedUncertain { .. } => {}
            }
        }
        failure.groups.push(group);
        stop
    }

    fn plans(
        scan: &CheckedSegment,
        selection: &OffloadOfferSelection,
        policy: &ReviewedPolicy,
        marks: &[Mark],
        today: Date,
        now: Timestamp,
    ) -> Result<Vec<PlannedMarkMutation>, OffloadOfferRefusal> {
        let inspection = inspect_grouped(
            scan,
            &selection.target,
            selection.layout,
            policy.policy(),
            today,
            now,
            DecisionPurpose::OffloadProposal,
        );
        if !matches!(inspection.empty().verdict(), GroupVerdict::Kept(_)) {
            return Ok(Vec::new());
        }
        let empty = scan
            .files()
            .iter()
            .filter(|file| {
                inspection
                    .empty()
                    .members()
                    .iter()
                    .any(|name| name == file.name.as_str())
            })
            .cloned()
            .collect::<Vec<_>>();
        // A policy keep is evaluated before terminal proof. Use its owning
        // predicate too; malformed or changed-size processing is never kept evidence.
        let RawRelease::Releasable(proven) = resolve(
            &ClosedHandlerSet,
            &JournalMedia,
            &selection.target.day,
            &selection.target.stream,
            &selection.target.dir,
            &empty,
        ) else {
            return Ok(Vec::new());
        };
        let kept = proven
            .iter()
            .map(|file| file.name())
            .collect::<BTreeSet<_>>();
        let mut plans = Vec::new();
        for mark in marks {
            let withdrawing = mark
                .proposal
                .names
                .iter()
                .filter(|name| kept.contains(name.as_str()))
                .filter(|name| {
                    mark.offload.as_ref().is_some_and(|offer| {
                        offer
                            .proof
                            .files
                            .iter()
                            .find(|file| &file.name == *name)
                            .is_some_and(|proof| {
                                scan.files()
                                    .iter()
                                    .find(|file| file.name.as_str() == name.as_str())
                                    .is_some_and(|file| {
                                        proof.bytes == file.size
                                            && proof.processing.record == file.sidecar.record
                                            && proof.processing.has_analysis_row
                                                == file.sidecar.has_analysis_row
                                    })
                            })
                    })
                })
                .collect::<BTreeSet<_>>();
            if withdrawing.is_empty() {
                continue;
            }
            let remaining = mark
                .proposal
                .names
                .iter()
                .filter(|name| !withdrawing.contains(name))
                .cloned()
                .collect::<Vec<_>>();
            let plan = PlannedMarkMutation::remainder(mark, &remaining).map_err(|error| {
                OffloadOfferRefusal::Review(crate::review_context::ReviewRefusal::Register(error))
            })?;
            plans.push(plan);
        }
        Ok(plans)
    }

    fn reconcile_group(
        operation: &PreparedArchiveOperation<'_>,
        selection: &OffloadOfferSelection,
        marks: &[Mark],
        today: Date,
        now: Timestamp,
        hooks: &mut Hooks,
    ) -> Result<OffloadReconciliationDisposition, OffloadOfferRefusal> {
        let claimed = super::super::acquire_claims(
            operation.journal_root(),
            std::slice::from_ref(selection),
        )?;
        let segment = claimed.first().ok_or(OffloadOfferRefusal::Changed)?;
        let absolute = &segment.absolute;
        let commit = segment.claims.commit(absolute)?;
        commit.require_outputs(
            &segment.claims,
            absolute,
            marks
                .iter()
                .flat_map(|mark| &mark.proposal.names)
                .map(|name| absolute.join(name).with_extension("jsonl")),
        )?;
        let policy = ReviewedPolicy::capture(operation.journal_root())
            .map_err(|error| OffloadOfferRefusal::Policy(Box::new(error)))?;
        let parent_relative = crate::layout::stream_rel_with_layout(
            &selection.target.day,
            &selection.target.stream,
            selection.layout,
        );
        let parent = MediaDirectory::open(
            Arc::clone(operation.journal_root()),
            Path::new(&parent_relative),
        )?;
        let owner = RetainedOwnerDeletion::open(
            parent,
            OsStr::new(&selection.target.dir),
            Some(&segment.directory),
            4096,
        )
        .map_err(io::Error::other)?;
        let scan = super::super::scan(&segment.directory)?;
        let plans = plans(&scan, selection, &policy, marks, today, now)?;
        let observed = ReconciliationObservation {
            operation,
            directory: &segment.directory,
            owner: &owner,
            scan: &scan,
            claims: &segment.claims,
            commit: &commit,
            policy: &policy,
            expected: marks,
            plans,
        };
        observed.revalidate()?;
        if observed.plans.is_empty() {
            return Ok(OffloadReconciliationDisposition::Unchanged);
        }
        hooks.before_publish();
        match crate::mark_mutation::reconcile_empty_observed(&observed) {
            Ok((outcome, observation)) => Ok(OffloadReconciliationDisposition::Publication {
                outcome: Box::new(outcome),
                observation: observation.map(Box::new),
            }),
            Err(ReconciliationWriteRefusal::Observation(error)) => Err(error),
            Err(ReconciliationWriteRefusal::Register(error)) => Ok(
                OffloadReconciliationDisposition::WriteRefused(Box::new(error)),
            ),
        }
    }

    pub(crate) enum ReconciliationWriteRefusal {
        Observation(OffloadOfferRefusal),
        Register(BoundMutationRefusal),
    }
    impl From<BoundMutationRefusal> for ReconciliationWriteRefusal {
        fn from(error: BoundMutationRefusal) -> Self {
            Self::Register(error)
        }
    }

    pub(super) fn reconcile(
        operation: &PreparedArchiveOperation<'_>,
        today: Date,
        now: Timestamp,
        hooks: &mut Hooks,
    ) -> Result<OffloadReconciliation, OffloadReconciliationFailure> {
        let mut failure = OffloadReconciliationFailure {
            root: Arc::clone(operation.journal_root()),
            last_confirmed: None,
            groups: Vec::new(),
            reason: OffloadReconciliationStop::Publication,
        };
        let observation = |error| OffloadReconciliationStop::Observation(Box::new(error));
        if let Err(error) = operation.revalidate() {
            failure.reason = observation(OffloadOfferRefusal::Admission(error));
            return Err(failure);
        }
        let initial = match crate::review_context::load_at_root(operation.journal_root()) {
            Ok(register) => register,
            Err(error) => {
                failure.reason = observation(OffloadOfferRefusal::Review(error));
                return Err(failure);
            }
        };
        failure.last_confirmed = Some(initial.clone());
        let mut groups: BTreeMap<String, (OffloadOfferSelection, Vec<Mark>)> = BTreeMap::new();
        for mark in initial.marks.values().filter(|mark| {
            mark.class == RemovalClass::OffloadRawRelease && matches!(mark.state, MarkState::Marked)
        }) {
            if mark.offload.is_none() {
                let withdrawn = OffloadReconciliationGroup {
                    selection: None,
                    expected: vec![mark.clone()],
                    disposition: withdraw_unbound(operation, mark),
                };
                if settle(&mut failure, withdrawn) {
                    return Err(failure);
                }
                continue;
            }
            match binding(operation.journal_root(), mark) {
                Ok(selection) => {
                    let group = groups
                        .entry(super::super::selection_path(&selection))
                        .or_insert_with(|| (selection.clone(), Vec::new()));
                    group.1.push(mark.clone());
                }
                Err(error) => failure.groups.push(OffloadReconciliationGroup {
                    selection: None,
                    expected: vec![mark.clone()],
                    disposition: OffloadReconciliationDisposition::Refused(Box::new(error)),
                }),
            }
        }
        for (_, (mut selection, marks)) in groups {
            selection.names = marks
                .iter()
                .flat_map(|mark| mark.proposal.names.clone())
                .collect::<BTreeSet<_>>()
                .into_iter()
                .collect();
            let disposition =
                match reconcile_group(operation, &selection, &marks, today, now, hooks) {
                    Ok(disposition) => disposition,
                    Err(OffloadOfferRefusal::Io(error))
                        if error.kind() == io::ErrorKind::WouldBlock =>
                    {
                        OffloadReconciliationDisposition::Busy
                    }
                    Err(error) => OffloadReconciliationDisposition::Refused(Box::new(error)),
                };
            let group = OffloadReconciliationGroup {
                selection: Some(selection),
                expected: marks,
                disposition,
            };
            if settle(&mut failure, group) {
                return Err(failure);
            }
        }
        if let Err(error) = operation.revalidate() {
            failure.reason = observation(OffloadOfferRefusal::Admission(error));
            return Err(failure);
        }
        let register = match crate::review_context::load_at_root(operation.journal_root()) {
            Ok(register) => register,
            Err(error) => {
                failure.reason = observation(OffloadOfferRefusal::Review(error));
                return Err(failure);
            }
        };
        Ok(OffloadReconciliation {
            root: failure.root,
            register,
            groups: failure.groups,
        })
    }

    #[cfg(all(test, unix, feature = "full-tests"))]
    pub(crate) fn reconcile_with_test_hook(
        operation: &PreparedArchiveOperation<'_>,
        today: Date,
        now: Timestamp,
        hook: Box<dyn FnMut()>,
    ) -> Result<OffloadReconciliation, OffloadReconciliationFailure> {
        reconcile(
            operation,
            today,
            now,
            &mut Hooks {
                before_publish: Some(hook),
            },
        )
    }
}
