// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (c) 2026 sol pbc

//! Owning offer verification retains the original read through publication and
//! settlement. Source-copy cleanup and final reservation retirement stay with
//! the caller that owns those sources and the outside runtime finish owner.

use solstone_core_backup::archive_readback::{
    OwnedDirectReadHold, OwnedReadOperation, OwnedSettledDirectRead,
};
use solstone_core_journal_io::media::{Cleanup, RootActivityCarrier};

use super::*;
use crate::archive_approval::{
    ArchiveApprovalError, ArchiveReadStates, OwnedArchiveApproval, OwnedArchiveApprovalBatch,
    OwnedArchiveApprovalFailure,
};
use crate::mark_mutation::BoundMutationOutcome;
use crate::remove_marked::{ArchiveReadState, ReviewedArchiveReadReport};

type Publication = Result<BoundMutationOutcome, OffloadOfferRefusal>;

enum OfferOwner {
    Pending(Box<OwnedReadOperation>),
    Failed(Box<OwnedArchiveApprovalFailure>),
}

/// Failed owning verification retains every original observation and resource.
#[must_use]
pub struct OwnedOffloadOfferFailure {
    observed: Box<ObservedOffloadOffers>,
    owner: OfferOwner,
    reason: OffloadOfferRefusal,
}

impl OwnedOffloadOfferFailure {
    pub fn reason(&self) -> &OffloadOfferRefusal {
        &self.reason
    }
    pub fn observed(&self) -> &ObservedOffloadOffers {
        &self.observed
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        match &self.owner {
            OfferOwner::Pending(read) => read.journal_root(),
            OfferOwner::Failed(failure) => failure.journal_root(),
        }
    }
    pub fn archive_reads(&self) -> impl Iterator<Item = ArchiveReadState<'_>> {
        match &self.owner {
            OfferOwner::Pending(_) => ArchiveReadStates::empty(),
            OfferOwner::Failed(failure) => failure.read_states(),
        }
    }
    pub fn into_owned_hold(self) -> OwnedOffloadOffersHold {
        let owner = match self.owner {
            OfferOwner::Pending(read) => {
                ClosedOfferOwner::Pending(Box::new(read.into_owned_hold()))
            }
            OfferOwner::Failed(failure) => {
                ClosedOfferOwner::Approval(Box::new(failure.into_owned_hold()))
            }
        };
        OwnedOffloadOffersHold {
            observed: self.observed,
            owner,
            reason: Some(Box::new(self.reason)),
            publication: None,
        }
    }
}

/// Actual owning sequential readback, with no public proof constructor.
#[must_use]
pub struct OwnedVerifiedOffloadOffers {
    observed: Box<ObservedOffloadOffers>,
    approval: OwnedArchiveApprovalBatch,
}

impl OwnedVerifiedOffloadOffers {
    pub fn observed(&self) -> &ObservedOffloadOffers {
        &self.observed
    }
    pub fn marks(&self) -> &[Mark] {
        self.approval.view().expected_marks()
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.approval.view().journal_root()
    }
    pub fn activity(&self) -> &Arc<ActivityLease> {
        self.approval.view().activity()
    }
    pub fn archive_reads(&self) -> impl Iterator<Item = ArchiveReadState<'_>> {
        self.approval.read_states()
    }
    pub fn into_owned_hold(self) -> OwnedOffloadOffersHold {
        OwnedOffloadOffersHold {
            observed: self.observed,
            owner: ClosedOfferOwner::Approval(Box::new(self.approval.into_owned_hold())),
            reason: None,
            publication: None,
        }
    }
}

/// Actual publication facts remain paired with their complete verification owner.
#[must_use]
pub struct OwnedOffloadOfferOutcome {
    verified: OwnedVerifiedOffloadOffers,
    publication: Publication,
}

impl OwnedOffloadOfferOutcome {
    pub fn verified(&self) -> &OwnedVerifiedOffloadOffers {
        &self.verified
    }
    pub fn publication(&self) -> &Publication {
        &self.publication
    }
    pub fn into_owned_hold(self) -> OwnedOffloadOffersHold {
        let mut held = self.verified.into_owned_hold();
        held.publication = Some(Box::new(self.publication));
        held
    }

    /// Settle only confirmed publication and the actual original read. The
    /// caller still owns source copies and the outside runtime finish owner.
    pub fn settle(self) -> Result<OwnedSettledOffloadOffers, OwnedOffloadOffersHold> {
        if !matches!(
            &self.publication,
            Ok(BoundMutationOutcome::Published(_) | BoundMutationOutcome::Unchanged(_))
        ) || !self.verified.approval.ready_for_finish()
        {
            return Err(self.into_owned_hold());
        }
        let Self {
            verified,
            publication,
        } = self;
        let OwnedVerifiedOffloadOffers { observed, approval } = verified;
        let marks = approval.view().expected_marks().to_vec();
        match approval.settle().release() {
            Ok((read, reads)) => Ok(OwnedSettledOffloadOffers {
                observed,
                read,
                report: SettledOffloadOfferReport {
                    publication,
                    marks,
                    reads,
                },
            }),
            Err(finishing) => Err(OwnedOffloadOffersHold {
                observed,
                owner: ClosedOfferOwner::Approval(Box::new(finishing.into_owned_hold())),
                reason: None,
                publication: Some(Box::new(publication)),
            }),
        }
    }
}

enum ClosedOfferOwner {
    Pending(Box<OwnedDirectReadHold>),
    Approval(Box<OwnedArchiveApproval>),
}

/// Closed custody; borrowed facts supply no execution or cleanup entrance.
#[must_use]
pub struct OwnedOffloadOffersHold {
    observed: Box<ObservedOffloadOffers>,
    owner: ClosedOfferOwner,
    reason: Option<Box<OffloadOfferRefusal>>,
    publication: Option<Box<Publication>>,
}

impl OwnedOffloadOffersHold {
    pub fn observed(&self) -> &ObservedOffloadOffers {
        &self.observed
    }
    pub fn carrier(&self) -> &Arc<RootActivityCarrier> {
        match &self.owner {
            ClosedOfferOwner::Pending(read) => read.carrier(),
            ClosedOfferOwner::Approval(approval) => approval.carrier(),
        }
    }
    pub fn journal_root(&self) -> &Arc<JournalRoot> {
        self.carrier().journal_root()
    }
    pub fn reason(&self) -> Option<&OffloadOfferRefusal> {
        self.reason.as_deref()
    }
    pub fn publication(&self) -> Option<&Publication> {
        self.publication.as_deref()
    }
    pub fn marks(&self) -> &[Mark] {
        match &self.owner {
            ClosedOfferOwner::Pending(_) => &[],
            ClosedOfferOwner::Approval(approval) => approval.expected_marks(),
        }
    }
    pub fn archive_reads(&self) -> impl Iterator<Item = ArchiveReadState<'_>> {
        match &self.owner {
            ClosedOfferOwner::Pending(_) => ArchiveReadStates::empty(),
            ClosedOfferOwner::Approval(approval) => approval.read_states(),
        }
    }
    fn approval_reason(&self) -> Option<&ArchiveApprovalError> {
        match &self.owner {
            ClosedOfferOwner::Pending(_) => None,
            ClosedOfferOwner::Approval(approval) => approval.reason(),
        }
    }
    pub fn readback_error(&self) -> Option<ReadbackError> {
        if let Some(ArchiveApprovalError::Readback(error)) = self.approval_reason() {
            return Some(*error);
        }
        match &self.owner {
            ClosedOfferOwner::Pending(_) => None,
            ClosedOfferOwner::Approval(approval) => approval.readback_error(),
        }
    }
    pub fn cleanup(&self) -> Option<Cleanup> {
        match self.approval_reason() {
            Some(ArchiveApprovalError::Cleanup(cleanup)) => Some(*cleanup),
            _ => None,
        }
    }
    pub fn native_error(&self) -> Option<&std::io::Error> {
        match self.approval_reason() {
            Some(ArchiveApprovalError::Native(error)) => Some(error),
            _ => None,
        }
    }
}

/// Resource-free offer and readback facts from actual successful settlement.
pub struct SettledOffloadOfferReport {
    publication: Publication,
    marks: Vec<Mark>,
    reads: Vec<ReviewedArchiveReadReport>,
}

impl SettledOffloadOfferReport {
    pub fn publication(&self) -> &Publication {
        &self.publication
    }
    pub fn marks(&self) -> &[Mark] {
        &self.marks
    }
    pub fn archive_reads(&self) -> &[ReviewedArchiveReadReport] {
        &self.reads
    }
}

/// The same settled read and original input observations await caller cleanup.
#[must_use]
pub struct OwnedSettledOffloadOffers {
    observed: Box<ObservedOffloadOffers>,
    read: OwnedSettledDirectRead,
    report: SettledOffloadOfferReport,
}

impl OwnedSettledOffloadOffers {
    pub fn observed(&self) -> &ObservedOffloadOffers {
        &self.observed
    }
    pub fn report(&self) -> &SettledOffloadOfferReport {
        &self.report
    }
    pub fn into_parts(
        self,
    ) -> (
        OwnedSettledDirectRead,
        ObservedOffloadOffers,
        SettledOffloadOfferReport,
    ) {
        (self.read, *self.observed, self.report)
    }
}

/// Verify through the same current-input predicates and actual sequential dumps.
/// Existing runtime cooperation remains a prerequisite for invocation.
pub fn verify_owned_offload_offers(
    read: OwnedReadOperation,
    observed: ObservedOffloadOffers,
    proofs: Vec<OffloadProof>,
    today: Date,
    now: Timestamp,
) -> Result<OwnedVerifiedOffloadOffers, OwnedOffloadOfferFailure> {
    let observed = Box::new(observed);
    #[cfg(any(unix, windows))]
    let candidates = candidate_marks(&observed, read.as_read(), proofs, now).and_then(|marks| {
        preflight_verification(read.as_read(), &observed, &marks, today, now)?;
        Ok(marks)
    });
    #[cfg(not(any(unix, windows)))]
    let candidates: Result<Vec<Mark>, OffloadOfferRefusal> = {
        let _parameters = (proofs, today, now);
        Err(OffloadOfferRefusal::Unsupported)
    };
    let marks = match candidates {
        Ok(marks) => marks,
        Err(reason) => {
            return Err(OwnedOffloadOfferFailure {
                observed,
                owner: OfferOwner::Pending(Box::new(read)),
                reason,
            });
        }
    };
    match crate::archive_approval::create_owned_archive_approval_batch(read, &marks) {
        Ok(approval) => Ok(OwnedVerifiedOffloadOffers { observed, approval }),
        Err(failure) => Err(OwnedOffloadOfferFailure {
            observed,
            owner: OfferOwner::Failed(Box::new(failure)),
            reason: OffloadOfferRefusal::Approval,
        }),
    }
}

/// The owning caller must retain confirmed witness-write receipts before entry.
/// Readable catalog history alone supplies no append-durability authority.
pub fn publish_owned_offload_offers(
    verified: OwnedVerifiedOffloadOffers,
    today: Date,
    now: Timestamp,
) -> OwnedOffloadOfferOutcome {
    #[cfg(any(unix, windows))]
    let publication =
        publish_with_approval(&verified.observed, verified.approval.view(), today, now);
    #[cfg(not(any(unix, windows)))]
    let publication = {
        let _parameters = (today, now);
        Err(OffloadOfferRefusal::Unsupported)
    };
    OwnedOffloadOfferOutcome {
        verified,
        publication,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owned_offer_custody_is_send_static() {
        fn check<T: Send + 'static>() {}
        fn active<T: 'static>() {}
        active::<OwnedOffloadOfferFailure>();
        active::<OwnedVerifiedOffloadOffers>();
        active::<OwnedOffloadOfferOutcome>();
        check::<OwnedOffloadOffersHold>();
        check::<OwnedSettledOffloadOffers>();
        check::<SettledOffloadOfferReport>();
    }
}
